//! Plugins on this server (`requirements/plugins.md`): installing and removing them, what the
//! admin grants them, and running one with exactly those grants.
//!
//! What is installed and granted lives in the database (`0006_plugins.sql`); each plugin's
//! code is `state/plugins/<id>.wasm`, kept compiled in memory once installed or first run, and
//! by wasmtime in `cache/plugins` across restarts. Plugin code runs on its own executor, never
//! on the threads serving requests (`design/plugins.md` §2).

pub mod grants;
mod hooks;
mod library;
pub mod personal;
pub mod rate_limits;
mod run;
pub mod settings;
#[cfg(test)]
pub mod testing;
mod turns;

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use jewelcase_plugins::{Host, InvalidPlugin, Manifest, Permission};
use jewelcase_scanner::Scanner;
use rusqlite::{Connection, params};
use tempfile::NamedTempFile;
use tokio::runtime::Runtime;
use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinError;

use crate::db::{Database, DbError, Json, now_ms};
use library::RunLibrary;
pub use run::RunResult;
use turns::Turns;

/// The plugin, library, hook, and user of a hook delivery.
type DeliveryKey = (String, i64, Permission, Option<i64>);

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("no such plugin")]
    NotFound,
    /// Not a plugin that can install, in words an admin can act on.
    #[error("{0}")]
    Invalid(String),
    #[error("{0} is already installed.")]
    Exists(String),
    /// Settings an admin entered that it will not take, in words they can act on.
    #[error("{0}")]
    SettingsInvalid(String),
    /// Required permissions not yet granted, named as the admin saw them.
    #[error("It still needs {}.", .0.join(" and "))]
    PermissionsRequired(Vec<&'static str>),
    #[error(transparent)]
    Db(#[from] DbError),
    /// A fault of the server's, in fixed words an admin can read. The cause is logged.
    #[error("{0}")]
    Internal(&'static str),
    /// A fault of the server's with nothing safe to say about it. The cause is logged.
    #[error("an unexpected error")]
    Unexpected,
}

impl From<rusqlite::Error> for PluginError {
    /// A database error, as [`PluginError::Db`].
    fn from(error: rusqlite::Error) -> PluginError {
        PluginError::Db(error.into())
    }
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
    /// `cache/plugins`, wasmtime's own, where it keeps them compiled across restarts, or `None`
    /// to compile them again after each restart.
    compiled: Option<PathBuf>,
    /// Each library's scanner, to pick up what a plugin saves.
    scanners: HashMap<i64, Scanner>,
    /// Each started on first use, the host by an install or a run and the runtime by a run, and
    /// idle between runs, so a server with no plugin enabled pays nothing for them.
    host: OnceLock<anyhow::Result<Arc<Host<RunLibrary>>>>,
    runtime: OnceLock<Result<Runtime, String>>,
    /// Each delivery under way, so each plugin, library, hook, and user has one at a time.
    delivering: Mutex<HashSet<DeliveryKey>>,
    /// Each plugin's turn to run in each library.
    turns: Turns,
    /// The hook deliveries that may run at once, one for each core, so a burst of them cannot
    /// hold a store each beside playback. Run now and settings checks, which someone is waiting
    /// on, take none.
    hook_slots: Semaphore,
    /// Wakes the hook loop, which sleeps while no plugin is enabled anywhere: notified when an
    /// admin enables one, and when the plugins are dropped.
    wake_hooks: Arc<Notify>,
}

impl Plugins {
    /// The plugins over `db`, installed in `dir`, kept compiled in `compiled` if given (a
    /// directory only the server's user can write, from `DataDir::private_cache`), with each
    /// library's scanner in `scanners` to pick up what a plugin saves.
    pub fn new(
        db: Arc<Database>,
        dir: PathBuf,
        compiled: Option<PathBuf>,
        scanners: HashMap<i64, Scanner>,
    ) -> Plugins {
        Plugins {
            db,
            dir,
            compiled,
            scanners,
            host: OnceLock::new(),
            runtime: OnceLock::new(),
            delivering: Mutex::default(),
            turns: Turns::default(),
            hook_slots: Semaphore::new(
                std::thread::available_parallelism().map_or(1, NonZeroUsize::get),
            ),
            wake_hooks: Arc::default(),
        }
    }

    /// The plugin host, started on first use. A host that could not start is logged and
    /// answered as an internal error, every time it is asked for.
    fn host(&self) -> Result<Arc<Host<RunLibrary>>, PluginError> {
        let host = self.host.get_or_init(|| Ok(Arc::new(Host::new(self.compiled.as_deref())?)));
        host.as_ref().map(Arc::clone).map_err(|error| {
            tracing::error!(error = %format_args!("{error:#}"), "cannot start the plugin host");
            PluginError::Internal("Plugins cannot run on this server.")
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
            PluginError::Internal("Plugins cannot run on this server.")
        })
    }

    /// Runs `task` on the plugin runtime and answers its output. The task goes on to its end even
    /// if the caller stops waiting for it, as a request does when its client gives up.
    async fn spawn<T: Send + 'static>(
        &self,
        task: impl Future<Output = T> + Send + 'static,
    ) -> Result<T, PluginError> {
        self.runtime()?.spawn(task).await.map_err(unexpected)
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.wasm"))
    }

    /// Installs the plugin at `url`.
    pub async fn install_url(&self, url: String) -> Result<String, PluginError> {
        let bytes = jewelcase_plugins::download(&url).await?;
        self.install(bytes, Some(url)).await
    }

    /// Installs the plugin in `bytes`, disabled in every library and with the grants it held
    /// before for requests it makes again unchanged (`requirements/plugins.md` §4.2, §5), kept
    /// compiled for its runs, and returns its ID. A file that could never run here, or a plugin
    /// installed already, is refused.
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
        let (checker, dir) = (host.clone(), self.dir.clone());
        // Compiling is CPU-bound: 10 ms for a Rust plugin, about a second for JavaScript. A file
        // that checks out is then staged where it goes, also off the async threads.
        let (linked, staged) = tokio::task::spawn_blocking(move || -> Result<_, PluginError> {
            let linked = checker.check(&bytes).map_err(|e| {
                PluginError::Invalid(format!("This plugin cannot run here: {e:#}."))
            })?;
            std::fs::create_dir_all(&dir).map_err(internal)?;
            let mut staged = NamedTempFile::new_in(&dir).map_err(internal)?;
            staged.write_all(&bytes).map_err(internal)?;
            staged.as_file().sync_all().map_err(internal)?;
            Ok((linked, staged))
        })
        .await
        .map_err(unexpected)??;
        let (target, id) = (self.path(&manifest.id), manifest.id.clone());
        self.db
            .try_write(move |tx| {
                add(tx, &manifest, source_url.as_deref())?;
                // Put in place within the transaction, so a row never names a missing file. A
                // staged file not put in place is deleted as it is dropped.
                staged.persist(&target).map(drop).map_err(|error| internal(error.into()))
            })
            .await?;
        host.installed(&id, linked);
        Ok(id)
    }

    /// Uninstalls `id`, once each of its runs under way has ended. Files it wrote stay, and so do
    /// its grants and settings, for a reinstall (`requirements/plugins.md` §5). Its users'
    /// connections go with it, and so do the requests to forget their searches meant for it, so
    /// after a reinstall, which starts every position over, users connect again and nothing from
    /// before is sent twice.
    pub async fn uninstall(&self, id: String) -> Result<(), PluginError> {
        // Its turn in every library and at the server level, taken in order and held until it is
        // gone, so no run of it is under way meanwhile, and one waiting finds it gone.
        let levels = self.db.read(|conn| {
            let sql = "SELECT 0 UNION SELECT id FROM libraries ORDER BY 1";
            conn.prepare_cached(sql)?
                .query_map([], |row| row.get(0))?
                .collect::<Result<Vec<i64>, _>>()
        });
        let mut turns = Vec::new();
        for level in levels.await? {
            turns.push(self.turns.take(&id, level).await);
        }
        let (path, plugin) = (self.path(&id), id.clone());
        let removed = self
            .db
            .write(move |tx| {
                tx.execute(
                    "DELETE FROM search_events WHERE kind = 'stopped' AND plugin_id = ?1",
                    [&id],
                )?;
                tx.execute("DELETE FROM plugins WHERE id = ?1", [&id])
            })
            .await?;
        if removed == 0 {
            return Err(PluginError::NotFound);
        }
        let file = tokio::task::spawn_blocking(move || std::fs::remove_file(path)).await;
        // A host not started yet has nothing linked to forget.
        if let Some(Ok(host)) = self.host.get() {
            host.uninstalled(&plugin);
        }
        match file.map_err(unexpected)? {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(internal(error)),
            _ => Ok(()),
        }
    }

    /// Replaces what `id` is granted ([`grants::set`]).
    pub async fn set_grants(
        &self,
        id: String,
        plugin_wide: Vec<Permission>,
        per_library: Vec<(i64, Vec<Permission>)>,
    ) -> Result<(), PluginError> {
        self.db
            .try_write(move |tx| {
                let manifest = grants::manifest(tx, &id)?.ok_or(PluginError::NotFound)?;
                Ok(grants::set(tx, &id, &manifest, &plugin_wide, &per_library)?)
            })
            .await
    }

    /// Enables or disables `id` in `library`. Enabling needs every required permission, and
    /// wakes the hook loop.
    pub async fn set_enabled(
        &self,
        id: String,
        library: i64,
        enabled: bool,
    ) -> Result<(), PluginError> {
        let missing = self
            .db
            .try_write(move |tx| -> Result<_, PluginError> {
                let manifest = grants::manifest(tx, &id)?.ok_or(PluginError::NotFound)?;
                Ok(grants::set_enabled(tx, &id, &manifest, library, enabled)?)
            })
            .await?;
        if !missing.is_empty() {
            let missing = missing.into_iter().map(Permission::title).collect();
            return Err(PluginError::PermissionsRequired(missing));
        }
        if enabled {
            self.wake_hooks.notify_one();
        }
        Ok(())
    }
}

impl Drop for Plugins {
    /// Ends the hook loop and the plugin runtime.
    fn drop(&mut self) {
        // The hook loop, woken, finds the plugins gone and ends.
        self.wake_hooks.notify_one();
        // Dropping a runtime waits for its threads, which is not allowed on an async thread.
        if let Some(Ok(runtime)) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// Records `manifest`'s plugin as installed from `source_url`, keeping only the grants it held
/// before whose requests it makes again unchanged ([`grants::keep_requested`]). Refused as
/// [`PluginError::Exists`], changing nothing, if a plugin with its ID is installed: the row is
/// never replaced, so what goes with it, such as its users' connections, goes only on a real
/// uninstall.
fn add(
    conn: &Connection,
    manifest: &Manifest,
    source_url: Option<&str>,
) -> Result<(), PluginError> {
    let inserted = conn.execute(
        "INSERT INTO plugins (id, manifest, source_url, installed_at) \
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT (id) DO NOTHING",
        params![manifest.id, Json(manifest), source_url, now_ms()],
    )?;
    if inserted == 0 {
        return Err(PluginError::Exists(manifest.name.clone()));
    }
    Ok(grants::keep_requested(conn, manifest)?)
}

#[cfg(test)]
mod tests;

/// [`PluginError::Internal`] for a plugin's file that could not be stored or removed, once
/// `error` is logged.
fn internal(error: std::io::Error) -> PluginError {
    tracing::error!(%error, "cannot store a plugin");
    PluginError::Internal("The plugin could not be stored.")
}

/// [`PluginError::Unexpected`] for a task that panicked or was cancelled, once `error` is logged.
fn unexpected(error: JoinError) -> PluginError {
    tracing::error!(%error, "a plugin task failed");
    PluginError::Unexpected
}
