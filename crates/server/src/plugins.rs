//! Plugins on this server (`requirements/plugins.md`): installing and removing them, what the
//! admin grants them, and running one with exactly those grants.
//!
//! What is installed and granted lives in the database (`0005_plugins.sql`); each plugin's
//! code is `state/plugins/<id>.wasm`. Plugin code runs on its own executor, never on the
//! threads serving requests (`design/plugins.md` §2).

pub mod grants;
mod hooks;
mod library;
mod run;
#[cfg(test)]
mod testing;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use jewelcase_plugins::{Host, InvalidPlugin, Permission};
use jewelcase_scanner::Scanner;
use rusqlite::params;
use tokio::runtime::Runtime;

use crate::db::{Database, DbError, now_ms};

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("no such plugin")]
    NotFound,
    /// Not a plugin that can install, in words an admin can act on.
    #[error("{0}")]
    Invalid(String),
    #[error("{0} is already installed.")]
    Exists(String),
    /// Required permissions not yet granted, named as the admin saw them.
    #[error("It still needs {}.", .0.join(" and "))]
    PermissionsRequired(Vec<&'static str>),
    #[error(transparent)]
    Db(#[from] DbError),
    /// The cause is logged; it can describe the host.
    #[error("{0}")]
    Internal(String),
}

impl From<InvalidPlugin> for PluginError {
    fn from(InvalidPlugin(message): InvalidPlugin) -> PluginError {
        PluginError::Invalid(message)
    }
}

pub struct Plugins {
    db: Arc<Database>,
    /// `state/plugins`, where installed components are kept.
    dir: PathBuf,
    /// Each library's scanner, to pick up what a plugin saves.
    scanners: HashMap<i64, Scanner>,
    /// Started on first use, so a server that never runs a plugin pays nothing for them.
    host: OnceLock<Result<Arc<Host>, String>>,
    runtime: OnceLock<Result<Runtime, String>>,
    /// The plugin, library, and hook of each delivery under way, so each has one at a time.
    delivering: Mutex<HashSet<(String, i64, &'static str)>>,
}

impl Plugins {
    pub fn new(db: Arc<Database>, dir: PathBuf, scanners: HashMap<i64, Scanner>) -> Plugins {
        Plugins {
            db,
            dir,
            scanners,
            host: OnceLock::new(),
            runtime: OnceLock::new(),
            delivering: Mutex::default(),
        }
    }

    fn host(&self) -> Result<Arc<Host>, PluginError> {
        self.host.get_or_init(|| Host::new().map(Arc::new)).clone().map_err(|error| {
            tracing::error!(%error, "cannot start the plugin host");
            PluginError::Internal("Plugins cannot run on this server.".into())
        })
    }

    fn runtime(&self) -> Result<&Runtime, PluginError> {
        // Built inside `get_or_init`, so racing requests never build a second one to drop.
        let runtime = self.runtime.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("plugins")
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
        });
        runtime.as_ref().map_err(|error| {
            tracing::error!(%error, "cannot start the plugin runtime");
            PluginError::Internal("Plugins cannot run on this server.".into())
        })
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.wasm"))
    }

    /// Installs the plugin at `url`.
    pub async fn install_url(&self, url: String) -> Result<String, PluginError> {
        let bytes = jewelcase_plugins::download(&url).await?;
        self.install(bytes, Some(url)).await
    }

    /// Installs the plugin in `bytes`, disabled in every library and with any grants it held
    /// before (`requirements/plugins.md` §5), and returns its ID. A file that could never run
    /// here is refused.
    pub async fn install(
        &self,
        bytes: Vec<u8>,
        source_url: Option<String>,
    ) -> Result<String, PluginError> {
        if bytes.len() > jewelcase_plugins::MAX_SIZE {
            return Err(PluginError::Invalid("The file is over 50 MB.".into()));
        }
        let manifest = jewelcase_plugins::manifest::read(&bytes)?;
        let host = self.host()?;
        // Compiling is CPU-bound: 10 ms for a Rust plugin, about a second for JavaScript.
        let (bytes, checked) = tokio::task::spawn_blocking(move || {
            let checked = host.check(&bytes);
            (bytes, checked)
        })
        .await
        .map_err(|e| PluginError::Internal(e.to_string()))?;
        checked.map_err(|e| PluginError::Invalid(format!("This plugin cannot run here: {e}.")))?;

        std::fs::create_dir_all(&self.dir).map_err(internal)?;
        let part = self.dir.join(format!("{}.{}.part", manifest.id, std::process::id()));
        std::fs::write(&part, &bytes).map_err(internal)?;
        let (target, id, name) =
            (self.path(&manifest.id), manifest.id.clone(), manifest.name.clone());
        let staged = part.clone();
        let installed = self
            .db
            .write(move |tx| {
                let json = serde_json::to_string(&manifest).expect("a manifest serializes");
                let now = now_ms();
                let inserted = tx.execute(
                    "INSERT INTO plugins (id, manifest, source_url, installed_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?4) ON CONFLICT (id) DO NOTHING",
                    params![manifest.id, json, source_url, now],
                )?;
                if inserted == 0 {
                    return Ok(false);
                }
                grants::keep_requested(tx, &manifest)?;
                // Put in place within the transaction, so a row never names a missing file.
                std::fs::rename(&staged, &target)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(true)
            })
            .await;
        let installed = installed.inspect_err(|_| drop(std::fs::remove_file(&part)))?;
        if !installed {
            std::fs::remove_file(&part).ok();
            return Err(PluginError::Exists(name));
        }
        Ok(id)
    }

    /// Uninstalls `id`. Files it wrote stay, and so do its grants, for a reinstall
    /// (`requirements/plugins.md` §5).
    pub async fn uninstall(&self, id: String) -> Result<(), PluginError> {
        let path = self.path(&id);
        let removed =
            self.db.write(move |tx| tx.execute("DELETE FROM plugins WHERE id = ?1", [id])).await?;
        if removed == 0 {
            return Err(PluginError::NotFound);
        }
        std::fs::remove_file(path).or_else(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(internal(e)),
        })
    }

    /// Replaces what `id` is granted ([`grants::set`]).
    pub async fn set_grants(
        &self,
        id: String,
        plugin_wide: Vec<Permission>,
        per_library: Vec<(i64, Vec<Permission>)>,
    ) -> Result<(), PluginError> {
        self.db
            .write(move |tx| {
                let Some(manifest) = grants::manifest(tx, &id)? else { return Ok(false) };
                grants::set(tx, &id, &manifest, &plugin_wide, &per_library).map(|()| true)
            })
            .await?
            .then_some(())
            .ok_or(PluginError::NotFound)
    }

    /// Enables or disables `id` in `library`. Enabling needs every required permission.
    pub async fn set_enabled(
        &self,
        id: String,
        library: i64,
        enabled: bool,
    ) -> Result<(), PluginError> {
        let missing = self
            .db
            .write(move |tx| {
                let Some(manifest) = grants::manifest(tx, &id)? else { return Ok(None) };
                grants::set_enabled(tx, &id, &manifest, library, enabled).map(Some)
            })
            .await?
            .ok_or(PluginError::NotFound)?;
        if missing.is_empty() {
            Ok(())
        } else {
            Err(PluginError::PermissionsRequired(
                missing.into_iter().map(Permission::title).collect(),
            ))
        }
    }
}

impl Drop for Plugins {
    fn drop(&mut self) {
        // Dropping a runtime waits for its threads, which is not allowed on an async thread.
        if let Some(Ok(runtime)) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests;

fn internal(error: std::io::Error) -> PluginError {
    tracing::error!(%error, "cannot store a plugin");
    PluginError::Internal("The plugin could not be stored.".into())
}
