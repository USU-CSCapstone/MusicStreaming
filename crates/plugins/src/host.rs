//! Running a plugin (`design/plugins.md` §2, §7): one engine per server, a fresh instance per
//! run, and host imports that each check their own permission.
//!
//! Imports stay linked when their permission is declined, so a plugin with an optional one
//! still loads: the call answers "… was not granted", and `granted` lets it adapt up front.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::manifest::Permission;

mod http;
mod library;

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "plugin",
        imports: { default: async },
        exports: { default: async },
    });
}

use bindings::jewelcase::plugin::host;
use bindings::{Plugin, PluginPre};

pub use bindings::jewelcase::plugin::events::{Event, ScanFinished, TracksChanged};
pub use bindings::jewelcase::plugin::library::{Album, Artist, Track};

/// Compute a plugin may spend in one run, counted while its code runs.
const CPU_BUDGET: Duration = Duration::from_secs(60);
/// Wall-clock time for a whole run. Epoch deadlines stop while the host awaits a network
/// reply, so this is what bounds a plugin waiting on a slow service.
const WALL_BUDGET: Duration = Duration::from_secs(300);
const MEMORY_LIMIT: usize = 64 << 20;
/// Log lines kept from one run, so a chatty plugin cannot fill the host's memory.
const LOG_LINES: usize = 1000;

/// The one library a run is for, as the host reads it, and the plugin's own state for it.
pub trait Library: Send + 'static {
    /// Tracks in ID order after `after`, up to `limit`; empty past the end.
    fn tracks(
        &mut self,
        after: Option<u64>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Track>, String>> + Send;

    /// These tracks, in this order, leaving out any not in the library.
    fn get_tracks(
        &mut self,
        ids: Vec<u64>,
    ) -> impl Future<Output = Result<Vec<Track>, String>> + Send;

    fn albums(
        &mut self,
        after: Option<u64>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Album>, String>> + Send;

    fn artists(
        &mut self,
        after: Option<u64>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Artist>, String>> + Send;

    /// The library's roots: the ID a plugin knows each by, and where it is on this host.
    fn roots(&mut self) -> impl Future<Output = Result<Vec<(u64, PathBuf)>, String>> + Send;

    fn state_get(
        &mut self,
        key: String,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, String>> + Send;

    fn state_set(
        &mut self,
        key: String,
        value: Vec<u8>,
    ) -> impl Future<Output = Result<(), String>> + Send;

    fn state_delete(&mut self, key: String) -> impl Future<Output = Result<(), String>> + Send;
}

/// What a run may use: the permissions granted for its library, the hosts its manifest names
/// for the network, and its settings.
pub struct Grants {
    pub permissions: Vec<Permission>,
    pub destinations: Vec<String>,
    /// The settings in effect for the run, defaults included.
    pub settings: serde_json::Map<String, serde_json::Value>,
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
    /// Each plugin compiled once, until its file changes: compiling is what costs, and hooks
    /// run a plugin often.
    compiled: Mutex<HashMap<PathBuf, (SystemTime, Component)>>,
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
        Ok(Host { engine, compiled: Mutex::default() })
    }

    /// Refuses a component that could never run here: one that does not compile, or imports
    /// something the plugin world does not offer.
    pub fn check(&self, bytes: &[u8]) -> Result<(), String> {
        let component =
            Component::new(&self.engine, bytes).map_err(|e| e.root_cause().to_string())?;
        self.pre::<Refused>(&component).map(drop)
    }

    /// The plugin in `file`, compiled, from the cache while the file is unchanged.
    async fn compiled(&self, file: &Path) -> Result<Component, String> {
        let unloadable = |e: String| format!("the plugin could not be loaded: {e}");
        let modified = std::fs::metadata(file)
            .and_then(|m| m.modified())
            .map_err(|e| unloadable(e.to_string()))?;
        let cache = |compiled: &HashMap<_, (SystemTime, Component)>| {
            compiled.get(file).filter(|(at, _)| *at == modified).map(|(_, c)| c.clone())
        };
        if let Some(component) = cache(&self.compiled.lock().expect("cache lock")) {
            return Ok(component);
        }
        // Compiling is CPU-bound, up to a second for a JavaScript plugin.
        let (engine, path) = (self.engine.clone(), file.to_path_buf());
        let component = tokio::task::spawn_blocking(move || Component::from_file(&engine, path))
            .await
            .map_err(|e| unloadable(e.to_string()))?
            .map_err(|e| unloadable(e.root_cause().to_string()))?;
        let mut compiled = self.compiled.lock().expect("cache lock");
        compiled.insert(file.to_path_buf(), (modified, component.clone()));
        Ok(component)
    }

    /// Runs the plugin in `component` once for `library`, with exactly `grants`, to handle
    /// `event`.
    pub async fn run<L: Library>(
        &self,
        component: &Path,
        grants: Grants,
        library: L,
        event: Event,
    ) -> Outcome {
        let loaded = self.compiled(component).await.and_then(|component| self.pre::<L>(&component));
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
        let result = call(&pre, &mut store, &event).await;
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

/// Instantiates the plugin and has it handle `event`, within the wall-clock budget.
async fn call<L: Library>(
    pre: &PluginPre<Run<L>>,
    store: &mut Store<Run<L>>,
    event: &Event,
) -> Result<String, String> {
    let plugin = pre.instantiate_async(&mut *store).await.map_err(|e| format!("{e:?}"))?;
    match tokio::time::timeout(WALL_BUDGET, plugin.call_handle(&mut *store, event)).await {
        Err(_) => Err(format!("the plugin ran past its {} s time limit", WALL_BUDGET.as_secs())),
        Ok(Err(trap)) => Err(format!("the plugin crashed: {}", trap.root_cause())),
        Ok(Ok(Err(reported))) => Err(format!("the plugin reported an error: {reported}")),
        Ok(Ok(Ok(summary))) => Ok(summary),
    }
}

/// A library nothing can be read from, for linking a component to check it.
struct Refused;

impl Library for Refused {
    async fn tracks(&mut self, _: Option<u64>, _: u32) -> Result<Vec<Track>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn get_tracks(&mut self, _: Vec<u64>) -> Result<Vec<Track>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn albums(&mut self, _: Option<u64>, _: u32) -> Result<Vec<Album>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn artists(&mut self, _: Option<u64>, _: u32) -> Result<Vec<Artist>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn roots(&mut self) -> Result<Vec<(u64, PathBuf)>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn state_get(&mut self, _: String) -> Result<Option<Vec<u8>>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn state_set(&mut self, _: String, _: Vec<u8>) -> Result<(), String> {
        Err(NO_LIBRARY.into())
    }

    async fn state_delete(&mut self, _: String) -> Result<(), String> {
        Err(NO_LIBRARY.into())
    }
}

const NO_LIBRARY: &str = "no library";

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
        Permission::TracksChanged => host::Permission::TracksChanged,
        Permission::ScanFinished => host::Permission::ScanFinished,
        Permission::Schedule => host::Permission::Schedule,
    }
}

impl<L: Library> bindings::jewelcase::plugin::settings::Host for Run<L> {
    async fn get(&mut self, name: String) -> Option<bindings::jewelcase::plugin::settings::Value> {
        use bindings::jewelcase::plugin::settings::Value;
        match self.grants.settings.get(&name)? {
            serde_json::Value::String(text) => Some(Value::Text(text.clone())),
            serde_json::Value::Number(n) => n.as_f64().map(Value::Number),
            serde_json::Value::Bool(flag) => Some(Value::Flag(*flag)),
            _ => None,
        }
    }
}

/// `events` has only types, which the plugin receives; it has nothing to call.
impl<L: Library> bindings::jewelcase::plugin::events::Host for Run<L> {}

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
