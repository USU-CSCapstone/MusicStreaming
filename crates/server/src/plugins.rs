//! Plugins on this server (`requirements/plugins.md`): installing and removing them, what the
//! admin grants them, and running one with exactly those grants.
//!
//! What is installed and granted lives in the database (`0005_plugins.sql`); each plugin's
//! code is `state/plugins/<id>.wasm`. Plugin code runs on its own executor, never on the
//! threads serving requests (`design/plugins.md` §2).

pub mod grants;
mod library;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use jewelcase_plugins::{Grants, Host, InvalidPlugin, Manifest, Permission};
use jewelcase_scanner::{Scanner, Trigger};
use rusqlite::params;
use tokio::runtime::Runtime;

use crate::db::{Database, DbError, now_ms};
use library::RunLibrary;

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

/// How a run went, as the admin sees it.
pub struct RunResult {
    pub ok: bool,
    pub summary: String,
    pub log: Vec<String>,
    /// Files it saved into the library.
    pub saved: usize,
    /// Whether a scan was queued to pick those files up.
    pub scanning: bool,
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
}

impl Plugins {
    pub fn new(db: Arc<Database>, dir: PathBuf, scanners: HashMap<i64, Scanner>) -> Plugins {
        Plugins { db, dir, scanners, host: OnceLock::new(), runtime: OnceLock::new() }
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

    /// Runs `id` once in each library it is enabled in, then queues scans of the folders it
    /// saved into: the scanner is the only way in (`requirements/general.md` §3.2).
    pub async fn run(&self, id: String) -> Result<RunResult, PluginError> {
        let plan = {
            let id = id.clone();
            self.db.read(move |conn| plan(conn, &id)).await?.ok_or(PluginError::NotFound)?
        };
        let (manifest, libraries) = plan;
        if libraries.is_empty() {
            return Ok(RunResult::failed(format!(
                "{} is not enabled in any library.",
                manifest.name
            )));
        }
        let (host, runtime) = (self.host()?, self.runtime()?);
        let mut result = RunResult {
            ok: true,
            summary: String::new(),
            log: Vec::new(),
            saved: 0,
            scanning: false,
        };
        for (library, permissions) in libraries {
            let missing = grants::missing(&manifest, &permissions);
            if !missing.is_empty() {
                let names: Vec<_> = missing.into_iter().map(Permission::title).collect();
                return Ok(RunResult::failed(format!("It still needs {}.", names.join(" and "))));
            }
            let grants = Grants { permissions, destinations: manifest.destinations().to_vec() };
            let (host, path, db, plugin_id) =
                (host.clone(), self.path(&id), self.db.clone(), id.clone());
            let outcome = runtime
                .spawn(async move {
                    host.run(&path, grants, RunLibrary::new(db, &plugin_id, library)).await
                })
                .await
                .map_err(|e| PluginError::Internal(e.to_string()))?;
            result.scanning |= self.scan(library, &outcome.touched).await;
            result.ok &= outcome.ok;
            result.saved += outcome.written;
            result.log.extend(outcome.log);
            if !result.summary.is_empty() {
                result.summary.push(' ');
            }
            result.summary.push_str(&outcome.summary);
        }
        Ok(result)
    }

    /// Queues a scan of each folder the paths in `touched` are in, and says whether any was
    /// queued. Queuing
    /// records the scan with a blocking write, which must not run on an async thread: there
    /// it would panic while holding the scanner's queue, and take that library's scanning down.
    async fn scan(&self, library: i64, touched: &[PathBuf]) -> bool {
        let Some(scanner) = self.scanners.get(&library).cloned() else { return false };
        let folders: BTreeSet<PathBuf> =
            touched.iter().filter_map(|file| file.parent()).map(Path::to_path_buf).collect();
        tokio::task::spawn_blocking(move || {
            // Every folder is queued, so none is skipped because an earlier one succeeded.
            let queued = folders.iter().map(|folder| scanner.scan_folder(Trigger::Watch, folder));
            queued.fold(false, |any, scan| any | scan.is_some())
        })
        .await
        .unwrap_or(false)
    }
}

impl RunResult {
    fn failed(summary: String) -> RunResult {
        RunResult { ok: false, summary, log: Vec::new(), saved: 0, scanning: false }
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

/// The manifest of `id`, and each library it is enabled in with what is granted there.
type Plan = (Manifest, Vec<(i64, Vec<Permission>)>);

fn plan(conn: &rusqlite::Connection, id: &str) -> rusqlite::Result<Option<Plan>> {
    let Some(manifest) = grants::manifest(conn, id)? else { return Ok(None) };
    let libraries: Vec<i64> = conn
        .prepare_cached(
            "SELECT library_id FROM plugin_libraries WHERE plugin_id = ?1 AND enabled \
             ORDER BY library_id",
        )?
        .query_map([id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let libraries = libraries
        .into_iter()
        .map(|library| Ok((library, grants::granted(conn, id, library)?)))
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some((manifest, libraries)))
}

#[cfg(test)]
mod tests;

fn internal(error: std::io::Error) -> PluginError {
    tracing::error!(%error, "cannot store a plugin");
    PluginError::Internal("The plugin could not be stored.".into())
}
