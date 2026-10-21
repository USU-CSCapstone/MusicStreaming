//! Running a plugin (`design/plugins.md` §2, §7): one engine per server, a fresh instance per
//! run, and host imports that each check their own permission.
//!
//! Imports stay linked when their permission is declined, so a plugin with an optional one
//! still loads: the call answers `not-granted`, and `granted` lets it adapt up front.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::manifest::{Permission, RateLimit};
use crate::network::Network;

mod http;
mod library;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

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

pub use bindings::jewelcase::plugin::events::{
    Changes, Event, Found, Play, PlayEnd, Playing, ResultKind, ScanFinished, Search, Selected,
};
pub use bindings::jewelcase::plugin::library::{Album, Artist, Track};
pub(crate) use bindings::jewelcase::plugin::{files::Entry, host::Error};

/// How often the epoch advances, and so how often a running plugin is checked. A tick is a sleep
/// that runs a little long, at about 1.05 ms.
const TICK: Duration = Duration::from_millis(1);
/// Compute a plugin may spend in one run, in ticks: about 63 s. Ticks count only while its code
/// runs, but each host call it waits on costs one however long it takes, so this is approximate.
const CPU_BUDGET: u64 = 60_000;
/// Wall-clock time for a whole run, waiting included, which bounds a plugin waiting on a slow
/// service.
const WALL_BUDGET: Duration = Duration::from_secs(300);
/// How large a run's linear memory may grow, and separately its wasm GC heap, which `MEMORIES`
/// does not count: about 128 MiB a run, before its tables and host-side state.
const MEMORY_LIMIT: usize = 64 << 20;
/// Linear memories a run may create. The bundled plugins and a JavaScript one built with
/// componentize-js 0.23 each have one; a composed component, with one per part, is refused.
const MEMORIES: usize = 1;
/// Tables a run may create in all. The bundled plugins and that JavaScript one each have 2.
const TABLES: usize = 10;
/// Core instances a run may create in all. The bundled plugins have 3, and that JavaScript one 4.
const INSTANCES: usize = 20;
/// Elements any one table may hold. The most measured is 8,989, in that JavaScript plugin built
/// ahead of time, and ten full tables of 8-byte elements come to 8 MB.
const TABLE_ELEMENTS: usize = 100_000;
/// Entries a run's WASI resource table may hold at once; a clock's pollable takes two. Handles
/// to resources the plugin defines itself are not counted here or by any other cap.
const RESOURCES: usize = 10_000;
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

    /// These albums, in this order, leaving out any not in the library.
    fn get_albums(
        &mut self,
        ids: Vec<u64>,
    ) -> impl Future<Output = Result<Vec<Album>, String>> + Send;

    /// These artists, in this order, leaving out any not in the library.
    fn get_artists(
        &mut self,
        ids: Vec<u64>,
    ) -> impl Future<Output = Result<Vec<Artist>, String>> + Send;

    /// The album's tracks, by disc and number; none for an album not in the library.
    fn album_tracks(
        &mut self,
        album: u64,
    ) -> impl Future<Output = Result<Vec<Track>, String>> + Send;

    /// The albums the artist is an album artist of, oldest first; none for an artist not in the
    /// library.
    fn artist_albums(
        &mut self,
        artist: u64,
    ) -> impl Future<Output = Result<Vec<Album>, String>> + Send;

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
    /// The plugin and library the run is for, which scope what it shares with other runs.
    pub plugin: String,
    pub library: i64,
    /// The user it acts for, if it acts for one, whose `"per": "user"` rate limits it keeps to.
    pub user: Option<i64>,
    pub permissions: Vec<Permission>,
    pub destinations: Vec<String>,
    /// The rate limits its manifest declares.
    pub rate_limits: Vec<RateLimit>,
    /// The hosts every plugin's requests keep to a limit for, lowercase, and the gap that limit
    /// keeps between them, leaving out those this plugin is exempt from.
    pub host_limits: Vec<(String, Duration)>,
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
    /// Whether a request found its service out of reach: held back as `rate-limited`,
    /// `timed-out`, unable to connect, or answered `429` or a `5xx`. A run that failed after one
    /// may have failed only for that, so the server retries it without counting it among the
    /// plugin's own failures.
    pub outage: bool,
}

pub struct Host {
    engine: Engine,
    /// Each plugin compiled once, until its file changes: compiling is what costs, and hooks
    /// run a plugin often.
    compiled: Mutex<HashMap<PathBuf, (SystemTime, Component)>>,
    /// What every run's requests share: each destination's pace, and replies to reuse.
    network: Arc<Network>,
    /// The ticks of compute each run may spend: `CPU_BUDGET`, or fewer in tests.
    budget: u64,
}

impl Host {
    /// The engine, and a thread advancing its epoch every `TICK`. The thread ends with the
    /// engine. Fails if either cannot be started.
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
                    thread::sleep(TICK);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Host { engine, compiled: Mutex::default(), network: Arc::default(), budget: CPU_BUDGET })
    }

    /// Refuses a component that could never run here: one that does not compile, instantiates
    /// core modules it imports, needs more memories, tables, or table elements than a run may
    /// create, or does not fit the contract.
    pub fn check(&self, bytes: &[u8]) -> Result<(), String> {
        let component =
            Component::new(&self.engine, bytes).map_err(|e| e.root_cause().to_string())?;
        fits(&component)?;
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
        let compiled = self.compiled(component).await;
        let loaded = compiled.and_then(|component| self.pre::<L>(&component));
        let pre = match loaded {
            Ok(pre) => pre,
            Err(summary) => {
                return Outcome {
                    ok: false,
                    summary,
                    log: Vec::new(),
                    written: 0,
                    touched: Vec::new(),
                    outage: false,
                };
            }
        };
        let mut store = self.store(grants, library);
        let result = call(&pre, &mut store, &event).await;
        // The store owned the run's state while the plugin ran; what it logged and saved is
        // read back from it.
        let run = store.into_data();
        let (ok, summary) = match result {
            Ok(summary) => (true, summary),
            Err(error) => (false, error),
        };
        let (log, written, touched, outage) = (run.log, run.written, run.touched, run.outage);
        Outcome { ok, summary, log, written, touched, outage }
    }

    /// A fresh store for one run over `library` with `grants`, with its caps and compute budget.
    fn store<L: Library>(&self, grants: Grants, library: L) -> Store<Run<L>> {
        let mut table = ResourceTable::new();
        table.set_max_capacity(RESOURCES);
        let run = Run {
            wasi: WasiCtx::builder().build(),
            table,
            limits: StoreLimitsBuilder::new()
                .memory_size(MEMORY_LIMIT)
                .memories(MEMORIES)
                .tables(TABLES)
                .table_elements(TABLE_ELEMENTS)
                .instances(INSTANCES)
                .build(),
            ticks: 0,
            grants,
            library,
            client: None,
            network: self.network.clone(),
            roots: None,
            log: Vec::new(),
            written: 0,
            touched: Vec::new(),
            outage: false,
            timeout: http::TIMEOUT,
        };
        let mut store = Store::new(&self.engine, run);
        // Memories, tables, and instances are counted across the whole store, so the caps bound
        // every instance in a component together; sizes are capped for each memory and table.
        store.limiter(|run| &mut run.limits);
        // Each deadline is one tick after the callback last ran, and is only checked while the
        // plugin's code runs, so a wait in a host call, however long, costs one tick.
        let budget = self.budget;
        store.epoch_deadline_callback(move |mut ctx| {
            let run = ctx.data_mut();
            run.ticks += 1;
            if run.ticks >= budget {
                return Err(wasmtime::Error::msg("the plugin ran past its compute budget"));
            }
            Ok(UpdateDeadline::Yield(1))
        });
        store.set_epoch_deadline(1);
        store
    }

    /// The component linked against the contract's bindings, ready to instantiate.
    fn pre<L: Library>(&self, component: &Component) -> Result<PluginPre<Run<L>>, String> {
        let mut linker = Linker::new(&self.engine);
        // WASI with nothing granted: no files, sockets, or environment. Only the imports below.
        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|e| format!("{e:?}"))?;
        Plugin::add_to_linker::<Run<L>, HasSelf<Run<L>>>(&mut linker, |run| run)
            .map_err(|e| format!("{e:?}"))?;
        // The whole chain, so a refused import is named, not only "implementation is missing".
        let pre = linker.instantiate_pre(component).map_err(|e| format!("{e:#}"))?;
        PluginPre::new(pre).map_err(|e| e.root_cause().to_string())
    }
}

/// Refuses a component whose core modules, all instantiated, would need more memories, tables,
/// or table elements than a run's store allows, so it is refused at install rather than on
/// every run.
fn fits(component: &Component) -> Result<(), String> {
    let needs = component
        .resources_required()
        .ok_or("the plugin instantiates core modules it imports, which a run cannot give it")?;
    if needs.num_memories as usize > MEMORIES {
        return Err(format!("the plugin has {} memories, and may have one", needs.num_memories));
    }
    if needs.num_tables as usize > TABLES {
        return Err(format!("the plugin has {} tables, and may have {TABLES}", needs.num_tables));
    }
    if let Some(elements) = needs.max_initial_table_size.filter(|&n| n > TABLE_ELEMENTS as u64) {
        return Err(format!(
            "the plugin has a table of {elements} elements, past {TABLE_ELEMENTS}"
        ));
    }
    Ok(())
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

    async fn get_albums(&mut self, _: Vec<u64>) -> Result<Vec<Album>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn get_artists(&mut self, _: Vec<u64>) -> Result<Vec<Artist>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn album_tracks(&mut self, _: u64) -> Result<Vec<Track>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn artist_albums(&mut self, _: u64) -> Result<Vec<Album>, String> {
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
    /// The ticks of compute it has spent.
    ticks: u64,
    grants: Grants,
    library: L,
    /// Made on the first request, so a plugin that never uses the network costs no client.
    client: Option<reqwest::Client>,
    network: Arc<Network>,
    /// Read on first use, and kept for the run.
    roots: Option<Vec<(u64, PathBuf)>>,
    log: Vec<String>,
    written: usize,
    touched: Vec<PathBuf>,
    /// Whether a request found its service out of reach ([`Outcome::outage`]).
    outage: bool,
    /// How long a request waits for its reply once sent.
    timeout: Duration,
}

impl<L: Library> Run<L> {
    /// Refuses as `not-granted` unless this run holds `permission`.
    fn may(&self, permission: Permission) -> Result<(), Error> {
        if self.grants.permissions.contains(&permission) { Ok(()) } else { Err(Error::NotGranted) }
    }

    /// The library's roots, read on first use. Failing to read them is a `failed`.
    async fn roots(&mut self) -> Result<&[(u64, PathBuf)], Error> {
        if self.roots.is_none() {
            self.roots = Some(self.library.roots().await.map_err(Error::Failed)?);
        }
        Ok(self.roots.as_deref().unwrap_or_default())
    }

    /// Where root `id` is on this host. An ID not among the library's roots is `refused`.
    async fn root(&mut self, id: u64) -> Result<PathBuf, Error> {
        let roots = self.roots().await?;
        let found = roots.iter().find(|(root, _)| *root == id);
        found
            .map(|(_, path)| path.clone())
            .ok_or_else(|| Error::Refused(format!("{id} is not one of this library's roots")))
    }
}

impl<L: Library> WasiView for Run<L> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

/// The permission as `host.granted` names it, or none for a hook, which a plugin is not told of.
fn wit(permission: Permission) -> Option<host::Permission> {
    match permission {
        Permission::LibraryRead => Some(host::Permission::LibraryRead),
        Permission::LibraryAdd => Some(host::Permission::LibraryAdd),
        Permission::LibraryChange => Some(host::Permission::LibraryChange),
        Permission::Network => Some(host::Permission::Network),
        Permission::ListeningActivity => Some(host::Permission::ListeningActivity),
        Permission::SearchActivity => Some(host::Permission::SearchActivity),
        Permission::HostRateLimits => Some(host::Permission::HostRateLimits),
        Permission::TracksChanged
        | Permission::ScanFinished
        | Permission::Schedule
        | Permission::Played
        | Permission::Playing
        | Permission::Searched
        | Permission::AlbumsChanged
        | Permission::ArtistsChanged => None,
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
        self.grants.permissions.iter().copied().filter_map(wit).collect()
    }

    async fn log(&mut self, message: String) {
        if self.log.len() < LOG_LINES {
            self.log.push(message.chars().take(500).collect());
        }
    }
}
