//! Running a plugin (`design/plugins.md` §2, §7): one engine per server, a fresh instance per
//! run, and host imports that each check their own permission.
//!
//! Imports stay linked when their permission is declined, so a plugin with an optional one
//! still loads: the call answers `not-granted`, and `granted` lets it adapt up front.

use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{
    Cache, CacheConfig, Config, Engine, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline,
    bail, ensure,
};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::manifest::{Permission, RateLimit};
use crate::network::Network;
use crate::rules::{self, Reach};
use epoch::Epoch;
use http::KeptClient;

mod compiled;
mod epoch;
mod http;
mod library;
mod outcome;
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
    Changes, Event, Forgotten, Found, Play, PlayEnd, Playing, ResultKind, ScanFinished, Search,
    Selected,
};
pub(crate) use bindings::jewelcase::plugin::files::{Entry, WriteMode};
pub(crate) use bindings::jewelcase::plugin::host::Error;
pub use bindings::jewelcase::plugin::library::{Album, Artist, Track};
pub use outcome::{Outcome, Refused, RunError};

/// Compute a plugin may spend in one run, in ticks: about 63 s. Ticks count only while its code
/// runs, but each host call it waits on costs one however long it takes, so this is approximate.
const CPU_BUDGET: u64 = 60_000;
/// Wall-clock time for handling an event, waiting included, which bounds a plugin waiting on a slow
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

    /// Albums in ID order after `after`, up to `limit`; empty past the end.
    fn albums(
        &mut self,
        after: Option<u64>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Album>, String>> + Send;

    /// Artists in ID order after `after`, up to `limit`; empty past the end.
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

    /// The value the plugin keeps under `key`, if it keeps one.
    fn state_get(
        &mut self,
        key: String,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, String>> + Send;

    /// Keeps `value` under `key`, in place of any kept there before.
    fn state_set(
        &mut self,
        key: String,
        value: Vec<u8>,
    ) -> impl Future<Output = Result<(), String>> + Send;

    /// Forgets the value under `key`, if there is one.
    fn state_delete(&mut self, key: String) -> impl Future<Output = Result<(), String>> + Send;
}

/// What a run may use: the permissions granted for its library, the hosts its manifest names
/// for the network, and its settings.
pub struct Grants {
    /// The plugin and library the run is for, which scope what it shares with other runs. The
    /// library is 0 for a run outside any library, which has no state.
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

/// Runs plugins over libraries read as an `L`.
pub struct Host<L: Library> {
    engine: Engine,
    /// Each installed plugin compiled and linked once, by ID.
    compiled: compiled::Cache<L>,
    /// What every run's requests share: each destination's pace, and replies to reuse.
    network: Arc<Network>,
    /// The ticks of compute each run may spend: `CPU_BUDGET`, or fewer in tests.
    budget: u64,
    /// Advances the engine's epoch while a run is live.
    epoch: Epoch,
}

impl<L: Library> Host<L> {
    /// The engine, and a thread advancing its epoch while a run is live, which ends with the
    /// host. With `cache`, an absolute directory for wasmtime's alone that no other user can
    /// write, compiled code is kept there too, so a restart loads it rather than compiling
    /// again. Fails if the engine, the thread, or the cache cannot be started.
    pub fn new(cache: Option<&Path>) -> wasmtime::Result<Host<L>> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        if let Some(directory) = cache {
            let mut settings = CacheConfig::new();
            settings.with_directory(directory);
            config.cache(Some(Cache::new(settings)?));
        }
        let engine = Engine::new(&config)?;
        let ticked = engine.clone();
        let epoch = Epoch::start(move || ticked.increment_epoch())?;
        let (compiled, network) = (Mutex::default(), Arc::default());
        Ok(Host { engine, compiled, network, budget: CPU_BUDGET, epoch })
    }

    /// Refuses a component that could never run here: one that does not compile, instantiates
    /// core modules it imports, needs more memories, tables, or table elements than a run may
    /// create, or does not fit the contract. Answers the component compiled and linked, for
    /// [`Host::installed`] once it is installed. Only a refusal's whole chain of causes (`{:#}`)
    /// is sure to say why, as with one from linking (`pre`).
    pub fn check(&self, bytes: &[u8]) -> wasmtime::Result<PluginPre<Run<L>>> {
        let component = Component::new(&self.engine, bytes)?;
        fits(&component)?;
        self.pre(&component)
    }

    /// Runs plugin `grants.plugin`, installed at `file`, once for `library`, with exactly
    /// `grants`, to handle `event`. One that cannot be compiled or linked is `Unloadable`, with
    /// every cause in its chain (`pre` says why).
    pub async fn run(&self, file: &Path, grants: Grants, library: L, event: Event) -> Outcome {
        let (pre, client) = match self.compiled(&grants.plugin, file).await {
            Ok(compiled) => compiled,
            Err(e) => return Outcome::failed(RunError::Unloadable(format!("{e:#}"))),
        };
        let mut store = self.store(grants, library, client);
        // Held until the run ends, so the epoch advances and its deadlines are enforced.
        let _live = self.epoch.live();
        let result = call(&pre, &mut store, &event).await;
        // The store owned the run's state while the plugin ran; what it logged, saved, was
        // refused, and spent is read back from it.
        let Run { log, written, touched, outage, refused, ticks, .. } = store.into_data();
        Outcome { result, log, written, touched, outage, refused, ticks }
    }

    /// A fresh store for one run over `library` with `grants`, with its caps and compute budget,
    /// and the plugin's `client` kept where its runs share it.
    fn store(&self, grants: Grants, library: L, client: Arc<KeptClient>) -> Store<Run<L>> {
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
            client,
            network: self.network.clone(),
            roots: None,
            log: Vec::new(),
            written: 0,
            touched: Vec::new(),
            outage: false,
            refused: Refused::default(),
            timeout: http::TIMEOUT,
            reach: rules::reach,
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
                return Err(RunError::OutOfCompute.into());
            }
            Ok(UpdateDeadline::Yield(1))
        });
        store.set_epoch_deadline(1);
        store
    }

    /// The component linked against the contract's bindings, ready to instantiate. An import it
    /// cannot be given is named only in the whole chain of the error's causes: the root cause
    /// says just "implementation is missing".
    fn pre(&self, component: &Component) -> wasmtime::Result<PluginPre<Run<L>>> {
        let mut linker = Linker::new(&self.engine);
        // WASI with nothing granted: no files, sockets, or environment. Only the imports below.
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        Plugin::add_to_linker::<Run<L>, HasSelf<Run<L>>>(&mut linker, |run| run)?;
        PluginPre::new(linker.instantiate_pre(component)?)
    }
}

/// Refuses a component whose core modules, all instantiated, would need more memories, tables,
/// or table elements than a run's store allows, so it is refused at install rather than on
/// every run.
fn fits(component: &Component) -> wasmtime::Result<()> {
    let Some(needs) = component.resources_required() else {
        bail!("the plugin instantiates core modules it imports, which a run cannot give it");
    };
    let (memories, tables) = (needs.num_memories as usize, needs.num_tables as usize);
    ensure!(memories <= MEMORIES, "the plugin has {memories} memories, and may have one");
    ensure!(tables <= TABLES, "the plugin has {tables} tables, and may have {TABLES}");
    if let Some(elements) = needs.max_initial_table_size.filter(|&n| n > TABLE_ELEMENTS as u64) {
        bail!("the plugin has a table of {elements} elements, past {TABLE_ELEMENTS}");
    }
    Ok(())
}

/// Instantiates the plugin and has it handle `event` within the wall-clock budget, answering
/// its summary or why the run failed.
async fn call<L: Library>(
    pre: &PluginPre<Run<L>>,
    store: &mut Store<Run<L>>,
    event: &Event,
) -> Result<String, RunError> {
    let plugin = pre.instantiate_async(&mut *store).await.map_err(RunError::trapped)?;
    let handled = tokio::time::timeout(WALL_BUDGET, plugin.call_handle(&mut *store, event)).await;
    let answered = handled.map_err(|_| RunError::OutOfTime)?;
    answered.map_err(RunError::trapped)?.map_err(RunError::Reported)
}

/// A run's state, which the plugin's host calls reach through the store.
pub struct Run<L> {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    /// The ticks of compute it has spent.
    ticks: u64,
    grants: Grants,
    library: L,
    /// Where the plugin's HTTP client is kept for all its runs (`compiled::Entry`).
    client: Arc<KeptClient>,
    network: Arc<Network>,
    /// Read on first use, and kept for the run.
    roots: Option<Vec<(u64, PathBuf)>>,
    log: Vec<String>,
    written: usize,
    touched: Vec<PathBuf>,
    /// Whether a request found its service out of reach ([`Outcome::outage`]).
    outage: bool,
    /// What the host refused it ([`Outcome::refused`]).
    refused: Refused,
    /// How long a request waits for its reply once sent.
    timeout: Duration,
    /// What each address is to the run's requests. A field rather than a call to `rules::reach`
    /// so that tests reach their servers on loopback, which no plugin may (`testing::reach`).
    reach: fn(IpAddr) -> Reach,
}

impl<L: Library> Run<L> {
    /// Refuses as `not-granted`, and records the refusal, unless this run holds `permission`.
    fn may(&mut self, permission: Permission) -> Result<(), Error> {
        if self.grants.permissions.contains(&permission) {
            return Ok(());
        }
        self.refused.add(permission.title());
        Err(Error::NotGranted)
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
    /// The run's WASI context, which grants nothing, and its resource table.
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
    /// The setting `name` in effect for the run, or none if it has no value the contract can
    /// carry: unset, or not text, a number, or a flag.
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
    /// The permissions this run holds, leaving out hooks.
    async fn granted(&mut self) -> Vec<host::Permission> {
        self.grants.permissions.iter().copied().filter_map(wit).collect()
    }

    /// Keeps `message`, cut to 500 characters, unless the run has logged [`LOG_LINES`] already.
    async fn log(&mut self, message: String) {
        if self.log.len() < LOG_LINES {
            self.log.push(message.chars().take(500).collect());
        }
    }
}
