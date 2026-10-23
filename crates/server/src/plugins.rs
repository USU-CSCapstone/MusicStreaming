//! Plugins on this server (`requirements/plugins.md`): installing and removing them, what the
//! admin grants them, and running one with exactly those grants.
//!
//! What is installed and granted lives in the database (`0006_plugins.sql`); each plugin's
//! code is `state/plugins/<id>.wasm`, kept compiled in memory once installed or first run, and
//! by wasmtime in `cache/plugins` across restarts. Plugin code runs on its own executor, never
//! on the threads serving requests (`design/plugins.md` §2).

pub mod grants;
mod hooks;
mod install;
mod library;
pub mod personal;
pub mod rate_limits;
mod run;
pub mod settings;
#[cfg(test)]
pub mod testing;
mod turns;

use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

use jewelcase_plugins::{Host, InvalidPlugin, Permission};
use jewelcase_scanner::Scanner;
use tokio::runtime::Runtime;
use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinError;

use crate::db::{Database, DbError};
use library::RunLibrary;
pub use run::RunResult;
use turns::Turns;

/// The plugin, library, hook, and user of a hook delivery.
type DeliveryKey = (String, i64, Permission, Option<i64>);

/// Why a plugin operation failed.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// No plugin is installed with that ID.
    #[error("no such plugin")]
    NotFound,
    /// Not a plugin that can install, in words an admin can act on.
    #[error("{0}")]
    Invalid(String),
    /// A plugin with that ID is installed already, named by its title.
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
    /// A file that is not a plugin, as [`PluginError::Invalid`] with its message.
    fn from(InvalidPlugin(message): InvalidPlugin) -> PluginError {
        PluginError::Invalid(message)
    }
}

/// The plugins installed on this server, and everything needed to install and run them.
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
    /// Set as the server stops, after which no hook delivery starts.
    hooks_stopped: AtomicBool,
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
            hooks_stopped: AtomicBool::new(false),
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

    /// The runtime plugin code runs on, one thread started on first use. A runtime that could
    /// not start is logged and answered as an internal error, every time it is asked for.
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

    /// Where `id`'s component is kept once installed.
    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.wasm"))
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

#[cfg(test)]
mod tests;

/// [`PluginError::Unexpected`] for a task that panicked or was cancelled, once `error` is logged.
fn unexpected(error: JoinError) -> PluginError {
    tracing::error!(%error, "a plugin task failed");
    PluginError::Unexpected
}
