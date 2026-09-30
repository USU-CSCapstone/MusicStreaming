//! Running a plugin (`design/plugins.md` §2, §7): one engine per server, a fresh instance per
//! run, and host imports that each check their own permission.
//!
//! Imports stay linked when their permission is declined, so a plugin with an optional one
//! still loads: the call answers "… was not granted", and `granted` lets it adapt up front.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::files as library_files;
use crate::manifest::Permission;
use crate::rules;

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "plugin",
        imports: { default: async },
        exports: { default: async },
    });
}

use bindings::jewelcase::plugin::{files, host, http, library};
use bindings::{Plugin, PluginPre};

pub use bindings::jewelcase::plugin::library::Track;

/// Compute a plugin may spend in one run, counted while its code runs.
const CPU_BUDGET: Duration = Duration::from_secs(60);
/// Wall-clock time for a whole run. Epoch deadlines stop while the host awaits a network
/// reply, so this is what bounds a plugin waiting on a slow service.
const WALL_BUDGET: Duration = Duration::from_secs(300);
const MEMORY_LIMIT: usize = 64 << 20;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const HTTP_MAX_BODY: usize = 2 << 20;
/// The most tracks one page holds, whatever the plugin asks for.
const PAGE_LIMIT: u32 = 500;
/// Log lines kept from one run, so a chatty plugin cannot fill the host's memory.
const LOG_LINES: usize = 1000;
const USER_AGENT: &str = concat!("Jewelcase/", env!("CARGO_PKG_VERSION"), " (plugin host)");

/// The one library a run is for, as the host reads it.
pub trait Library: Send + 'static {
    /// A page of tracks in a stable order; empty past the end.
    fn tracks(
        &mut self,
        offset: u32,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Track>, String>> + Send;

    /// The library's roots: the ID a plugin knows each by, and where it is on this host.
    fn roots(&mut self) -> impl Future<Output = Result<Vec<(u64, PathBuf)>, String>> + Send;
}

/// What a run may use: the permissions granted for its library, and the hosts its manifest
/// names for the network.
pub struct Grants {
    pub permissions: Vec<Permission>,
    pub destinations: Vec<String>,
}

/// How a run went, in the plugin's own words where it gave them.
pub struct Outcome {
    pub ok: bool,
    pub summary: String,
    pub log: Vec<String>,
    /// How many files it wrote.
    pub written: usize,
    /// Every path it created, replaced, moved, or deleted, for the scanner to look at again.
    pub touched: Vec<PathBuf>,
}

pub struct Host {
    engine: Engine,
}

impl Host {
    /// The engine, and a thread ticking its epoch so deadlines are checked every millisecond.
    /// The thread ends with the engine.
    pub fn new() -> Result<Host, String> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(|e| format!("{e:?}"))?;
        let weak = engine.weak();
        thread::Builder::new()
            .name("plugin-epoch".into())
            .spawn(move || {
                while let Some(engine) = weak.upgrade() {
                    engine.increment_epoch();
                    drop(engine);
                    thread::sleep(Duration::from_millis(1));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Host { engine })
    }

    /// Refuses a component that could never run here: one that does not compile, or imports
    /// something the plugin world does not offer.
    pub fn check(&self, bytes: &[u8]) -> Result<(), String> {
        let component =
            Component::new(&self.engine, bytes).map_err(|e| e.root_cause().to_string())?;
        self.pre::<Refused>(&component).map(drop)
    }

    /// Runs the plugin in `component` once for `library`, with exactly `grants`.
    pub async fn run<L: Library>(&self, component: &Path, grants: Grants, library: L) -> Outcome {
        let loaded = Component::from_file(&self.engine, component)
            .map_err(|e| format!("the plugin could not be loaded: {}", e.root_cause()))
            .and_then(|component| self.pre::<L>(&component));
        let pre = match loaded {
            Ok(pre) => pre,
            Err(summary) => {
                return Outcome {
                    ok: false,
                    summary,
                    log: Vec::new(),
                    written: 0,
                    touched: Vec::new(),
                };
            }
        };
        let run = Run {
            wasi: WasiCtx::builder().build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).build(),
            deadline: Instant::now() + CPU_BUDGET,
            grants,
            library,
            client: None,
            roots: None,
            log: Vec::new(),
            written: 0,
            touched: Vec::new(),
        };
        let mut store = Store::new(&self.engine, run);
        store.limiter(|run| &mut run.limits);
        store.epoch_deadline_callback(|ctx| {
            if Instant::now() >= ctx.data().deadline {
                return Err(wasmtime::Error::msg("the plugin ran past its compute budget"));
            }
            Ok(UpdateDeadline::Yield(1))
        });
        store.set_epoch_deadline(1);
        let result = call(&pre, &mut store).await;
        // The store owned the run's state while the plugin ran; what it logged and saved is
        // read back from it.
        let run = store.into_data();
        let (ok, summary) = match result {
            Ok(summary) => (true, summary),
            Err(error) => (false, error),
        };
        Outcome { ok, summary, log: run.log, written: run.written, touched: run.touched }
    }

    fn pre<L: Library>(&self, component: &Component) -> Result<PluginPre<Run<L>>, String> {
        let mut linker = Linker::new(&self.engine);
        // WASI with nothing granted: no files, sockets, or environment. Only the imports below.
        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|e| format!("{e:?}"))?;
        Plugin::add_to_linker::<Run<L>, HasSelf<Run<L>>>(&mut linker, |run| run)
            .map_err(|e| format!("{e:?}"))?;
        let pre = linker.instantiate_pre(component).map_err(|e| e.root_cause().to_string())?;
        PluginPre::new(pre).map_err(|e| e.root_cause().to_string())
    }
}

/// Instantiates the plugin and calls its `run`, within the wall-clock budget.
async fn call<L: Library>(
    pre: &PluginPre<Run<L>>,
    store: &mut Store<Run<L>>,
) -> Result<String, String> {
    let plugin = pre.instantiate_async(&mut *store).await.map_err(|e| format!("{e:?}"))?;
    match tokio::time::timeout(WALL_BUDGET, plugin.call_run(&mut *store)).await {
        Err(_) => Err(format!("the plugin ran past its {} s time limit", WALL_BUDGET.as_secs())),
        Ok(Err(trap)) => Err(format!("the plugin crashed: {}", trap.root_cause())),
        Ok(Ok(Err(reported))) => Err(format!("the plugin reported an error: {reported}")),
        Ok(Ok(Ok(summary))) => Ok(summary),
    }
}

/// A library nothing can be read from, for linking a component to check it.
struct Refused;

impl Library for Refused {
    async fn tracks(&mut self, _: u32, _: u32) -> Result<Vec<Track>, String> {
        Err("no library".into())
    }

    async fn roots(&mut self) -> Result<Vec<(u64, PathBuf)>, String> {
        Err("no library".into())
    }
}

/// A run's state, which the plugin's host calls reach through the store.
pub struct Run<L> {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    deadline: Instant,
    grants: Grants,
    library: L,
    /// Made on the first request, so a plugin that never uses the network costs no client.
    client: Option<reqwest::Client>,
    /// Read on first use, and kept for the run.
    roots: Option<Vec<(u64, PathBuf)>>,
    log: Vec<String>,
    written: usize,
    touched: Vec<PathBuf>,
}

impl<L: Library> Run<L> {
    fn may(&self, permission: Permission) -> Result<(), String> {
        if self.grants.permissions.contains(&permission) {
            Ok(())
        } else {
            Err(format!("{} was not granted", permission.title()))
        }
    }

    async fn roots(&mut self) -> Result<&[(u64, PathBuf)], String> {
        if self.roots.is_none() {
            self.roots = Some(self.library.roots().await?);
        }
        Ok(self.roots.as_deref().unwrap_or_default())
    }

    /// Where root `id` is on this host.
    async fn root(&mut self, id: u64) -> Result<PathBuf, String> {
        let roots = self.roots().await?;
        let found = roots.iter().find(|(root, _)| *root == id);
        found
            .map(|(_, path)| path.clone())
            .ok_or_else(|| format!("{id} is not one of this library's roots"))
    }
}

/// Runs file work off the plugin runtime's thread, so one plugin's disk waits hold up no other.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work).await.map_err(|_| "the file operation failed".to_owned())?
}

impl<L: Library> WasiView for Run<L> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

fn wit(permission: Permission) -> host::Permission {
    match permission {
        Permission::LibraryRead => host::Permission::LibraryRead,
        Permission::LibraryAdd => host::Permission::LibraryAdd,
        Permission::LibraryChange => host::Permission::LibraryChange,
        Permission::Network => host::Permission::Network,
        Permission::ListeningActivity => host::Permission::ListeningActivity,
    }
}

impl<L: Library> host::Host for Run<L> {
    async fn granted(&mut self) -> Vec<host::Permission> {
        self.grants.permissions.iter().copied().map(wit).collect()
    }

    async fn log(&mut self, message: String) {
        if self.log.len() < LOG_LINES {
            self.log.push(message.chars().take(500).collect());
        }
    }
}

impl<L: Library> library::Host for Run<L> {
    async fn tracks(&mut self, offset: u32, limit: u32) -> Result<Vec<Track>, String> {
        self.may(Permission::LibraryRead)?;
        self.library.tracks(offset, limit.min(PAGE_LIMIT)).await
    }
}

impl<L: Library> files::Host for Run<L> {
    async fn roots(&mut self) -> Result<Vec<u64>, String> {
        let file_permissions =
            [Permission::LibraryRead, Permission::LibraryAdd, Permission::LibraryChange];
        if !file_permissions.iter().any(|p| self.grants.permissions.contains(p)) {
            return Err(format!("{} was not granted", Permission::LibraryRead.title()));
        }
        Ok(Run::roots(self).await?.iter().map(|(id, _)| *id).collect())
    }

    async fn list(&mut self, root: u64, folder: String) -> Result<Vec<files::Entry>, String> {
        self.may(Permission::LibraryRead)?;
        let root = self.root(root).await?;
        let entries = blocking(move || library_files::list(&root, &folder)).await?;
        Ok(entries
            .into_iter()
            .map(|e| files::Entry {
                name: e.name,
                directory: e.directory,
                size: e.size,
                modified_ms: e.modified_ms,
            })
            .collect())
    }

    async fn read(
        &mut self,
        root: u64,
        path: String,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, String> {
        self.may(Permission::LibraryRead)?;
        let root = self.root(root).await?;
        blocking(move || library_files::read(&root, &path, offset, length)).await
    }

    async fn write(
        &mut self,
        root: u64,
        path: String,
        contents: Vec<u8>,
        mode: files::WriteMode,
    ) -> Result<(), String> {
        let replace = matches!(mode, files::WriteMode::Replace);
        self.may(if replace { Permission::LibraryChange } else { Permission::LibraryAdd })?;
        let root = self.root(root).await?;
        let written =
            blocking(move || library_files::write(&root, &path, &contents, replace)).await?;
        self.written += 1;
        self.touched.push(written);
        Ok(())
    }

    async fn rename(&mut self, root: u64, from: String, to: String) -> Result<(), String> {
        self.may(Permission::LibraryChange)?;
        let root = self.root(root).await?;
        let (from, to) = blocking(move || library_files::rename(&root, &from, &to)).await?;
        self.touched.extend([from, to]);
        Ok(())
    }

    async fn delete(&mut self, root: u64, path: String) -> Result<(), String> {
        self.may(Permission::LibraryChange)?;
        let root = self.root(root).await?;
        let deleted = blocking(move || library_files::delete(&root, &path)).await?;
        self.touched.push(deleted);
        Ok(())
    }
}

impl<L: Library> http::Host for Run<L> {
    async fn get(&mut self, url: String) -> Result<http::Response, String> {
        self.may(Permission::Network)?;
        rules::allowed(&url, &self.grants.destinations)?;
        let client = match &self.client {
            Some(client) => client.clone(),
            None => {
                let client = reqwest::Client::builder()
                    .user_agent(USER_AGENT)
                    .connect_timeout(Duration::from_secs(10))
                    .build()
                    .map_err(|e| format!("the network is unavailable: {e}"))?;
                self.client.insert(client).clone()
            }
        };
        let fetch = async {
            let mut res = client
                .get(&url)
                .send()
                .await
                .map_err(|e| format!("request failed: {}", e.without_url()))?;
            let status = res.status().as_u16();
            let mut body = Vec::new();
            while let Some(chunk) = res.chunk().await.map_err(|e| format!("reply failed: {e}"))? {
                body.extend_from_slice(&chunk);
                if body.len() > HTTP_MAX_BODY {
                    return Err("the reply is over 2 MB".to_owned());
                }
            }
            Ok(http::Response { status, body: String::from_utf8_lossy(&body).into_owned() })
        };
        tokio::time::timeout(HTTP_TIMEOUT, fetch)
            .await
            .map_err(|_| "the request timed out".to_owned())?
    }
}
