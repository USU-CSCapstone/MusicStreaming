//! The candidate: WebAssembly components hosted in-process by Wasmtime.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable, bindgen};
use wasmtime::{
    Config, Engine, InstanceAllocationStrategy, PoolingAllocationConfig, Store, StoreLimits,
    StoreLimitsBuilder, UpdateDeadline,
};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::library::{self, Scope};

bindgen!({ path: "../wit", world: "plugin", exports: { default: async } });

use jewelcase::spike::library as wit_library;

/// How often the engine's epoch advances: the granularity of deadlines, yields, and pauses.
/// 250 µs unless `SPIKE_TICK_US` says otherwise: at 1 ms, plugins spinning on every worker
/// held others back 3 ms at p99; at 250 µs, 1.5 ms; shorter ticks bought nothing more.
pub fn tick() -> Duration {
    static TICK: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *TICK.get_or_init(|| {
        let us = std::env::var("SPIKE_TICK_US").ok().and_then(|v| v.parse().ok()).unwrap_or(250);
        Duration::from_micros(us)
    })
}

/// A plugin call that ran past its budget (`requirements/plugins.md` §2.2).
#[derive(Debug)]
pub struct DeadlineExceeded;

impl std::fmt::Display for DeadlineExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("plugin call exceeded its deadline")
    }
}

impl std::error::Error for DeadlineExceeded {}

/// The Governor's role for plugins: background work parks while listeners need the machine
/// (`requirements/plugins.md` §2.2, `crates/scanner/src/governor.rs`).
#[derive(Default)]
pub struct Pause(AtomicBool);

impl Pause {
    pub fn set(&self, paused: bool) {
        self.0.store(paused, Ordering::SeqCst);
    }

    fn is_paused(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    async fn resumed(self: Arc<Self>) {
        while self.is_paused() {
            tokio::time::sleep(Duration::from_micros(200)).await;
        }
    }
}

pub struct HostState {
    wasi: WasiCtx,
    table: ResourceTable,
    scope: Scope,
    limits: StoreLimits,
    deadline: Instant,
    pause: Arc<Pause>,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

fn to_wit(t: library::Track) -> wit_library::Track {
    wit_library::Track { id: t.id, title: t.title, artist: t.artist, duration_us: t.duration_us }
}

impl wit_library::Host for HostState {
    fn tracks(&mut self, offset: u32, limit: u32) -> Vec<wit_library::Track> {
        self.scope.page(offset, limit).into_iter().map(to_wit).collect()
    }

    fn get_track(&mut self, id: u64) -> Option<wit_library::Track> {
        self.scope.get(id).map(to_wit)
    }
}

impl jewelcase::spike::state::Host for HostState {
    fn get(&mut self, key: String) -> Option<Vec<u8>> {
        self.scope.state_get(&key)
    }

    fn set(&mut self, key: String, value: Vec<u8>) {
        self.scope.state_set(key, value);
    }
}

pub struct Runtime {
    pub engine: Engine,
    linker: Linker<HostState>,
}

impl Runtime {
    /// `pooling` preallocates instance slots, the production choice for fast (re)instantiation.
    pub fn new(pooling: bool) -> Result<Runtime> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        if pooling {
            let mut pool = PoolingAllocationConfig::default();
            pool.total_component_instances(64);
            pool.total_core_instances(128);
            pool.total_memories(128);
            pool.total_tables(128);
            pool.max_memory_size(512 << 20);
            config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
        }
        let engine = Engine::new(&config)?;

        // One ticker per engine; it stops when the engine is dropped.
        let weak = engine.weak();
        thread::spawn(move || {
            while let Some(engine) = weak.upgrade() {
                engine.increment_epoch();
                drop(engine);
                thread::sleep(tick());
            }
        });

        let mut linker = Linker::new(&engine);
        // WASI with no preopens, no sockets, no inherited stdio: nothing beyond the scoped imports.
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        Plugin::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |s| s)?;
        Ok(Runtime { engine, linker })
    }

    /// Compiles a component and links it once; instances are then cheap to make.
    pub fn load(&self, path: &Path) -> Result<Loaded> {
        let component = Component::from_file(&self.engine, path)?;
        self.prepare(component)
    }

    pub fn prepare(&self, component: Component) -> Result<Loaded> {
        let pre = PluginPre::new(self.linker.instantiate_pre(&component)?)?;
        Ok(Loaded { pre })
    }

    pub async fn instantiate(&self, loaded: &Loaded, scope: Scope, opts: Opts) -> Result<Instance> {
        let state = HostState {
            wasi: WasiCtx::builder().build(),
            table: ResourceTable::new(),
            scope,
            limits: StoreLimitsBuilder::new().memory_size(opts.memory_limit).build(),
            deadline: Instant::now() + DEFAULT_BUDGET,
            pause: opts.pause,
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);
        store.epoch_deadline_callback(|ctx| {
            let s = ctx.data();
            if s.pause.is_paused() {
                return Ok(UpdateDeadline::YieldCustom(1, Box::pin(s.pause.clone().resumed())));
            }
            if Instant::now() >= s.deadline {
                return Err(wasmtime::Error::new(DeadlineExceeded));
            }
            // Yield every tick, so a long call never holds an executor thread others need.
            Ok(UpdateDeadline::Yield(1))
        });
        store.set_epoch_deadline(1);
        let plugin = loaded.pre.instantiate_async(&mut store).await?;
        Ok(Instance { store, plugin })
    }
}

pub struct Loaded {
    pre: PluginPre<HostState>,
}

pub const DEFAULT_BUDGET: Duration = Duration::from_secs(30);

pub struct Opts {
    pub memory_limit: usize,
    pub pause: Arc<Pause>,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { memory_limit: 256 << 20, pause: Arc::default() }
    }
}

/// A warm instance. After any error it must be discarded: a trapped component
/// cannot be re-entered.
pub struct Instance {
    store: Store<HostState>,
    plugin: Plugin,
}

impl Instance {
    fn arm(&mut self, budget: Duration) {
        self.store.data_mut().deadline = Instant::now() + budget;
        self.store.set_epoch_deadline(1);
    }

    pub async fn noop(&mut self) -> Result<()> {
        self.arm(DEFAULT_BUDGET);
        lift(self.plugin.call_noop(&mut self.store).await)
    }

    pub async fn echo(&mut self, s: &str) -> Result<String> {
        self.arm(DEFAULT_BUDGET);
        lift(self.plugin.call_echo(&mut self.store, s).await)
    }

    pub async fn scan_titles(&mut self, needle: &str, batch: u32, budget: Duration) -> Result<u32> {
        self.arm(budget);
        lift(self.plugin.call_scan_titles(&mut self.store, needle, batch).await)
    }

    pub async fn counter(&mut self) -> Result<u64> {
        self.arm(DEFAULT_BUDGET);
        lift(self.plugin.call_counter(&mut self.store).await)
    }

    pub async fn persisted_counter(&mut self) -> Result<u64> {
        self.arm(DEFAULT_BUDGET);
        lift(self.plugin.call_persisted_counter(&mut self.store).await)
    }

    pub async fn spin(&mut self, budget: Duration) -> Result<()> {
        self.arm(budget);
        lift(self.plugin.call_spin(&mut self.store).await)
    }

    pub async fn crash(&mut self) -> Result<()> {
        self.arm(DEFAULT_BUDGET);
        lift(self.plugin.call_crash(&mut self.store).await)
    }

    pub async fn hog(&mut self, mb: u32) -> Result<u32> {
        self.arm(DEFAULT_BUDGET);
        lift(self.plugin.call_hog(&mut self.store, mb).await)
    }
}

/// Wasmtime's error into the harness's, keeping a deadline recognizable after the conversion.
fn lift<T>(r: wasmtime::Result<T>) -> Result<T> {
    r.map_err(|e| {
        if e.downcast_ref::<DeadlineExceeded>().is_some() {
            anyhow!(DeadlineExceeded)
        } else {
            anyhow!("{e:?}")
        }
    })
}

pub fn is_deadline(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<DeadlineExceeded>().is_some())
}

#[cfg(test)]
mod tests {
    //! Quick versions of the properties the harness asserts. Needs the guests built (`build.sh`).

    use super::*;
    use crate::library::{BIG, Catalog, SMALL, StateStore};

    const RUST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/wasm32-wasip2/release/guest_rust.wasm");

    async fn setup() -> (Runtime, Loaded, Catalog) {
        let rt = Runtime::new(false).unwrap();
        let loaded = rt.load(Path::new(RUST)).expect("build the guests first: ./build.sh");
        (rt, loaded, Catalog::generate(300, 40))
    }

    fn scope(catalog: &Catalog, library: u64) -> Scope {
        Scope::new(catalog, library, "test", Arc::new(StateStore::default()))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn scope_bounds_what_a_plugin_sees() {
        let (rt, loaded, catalog) = setup().await;
        let mut small = rt.instantiate(&loaded, scope(&catalog, SMALL), Opts::default()).await.unwrap();
        let mut big = rt.instantiate(&loaded, scope(&catalog, BIG), Opts::default()).await.unwrap();
        assert_eq!(small.scan_titles("", 7, DEFAULT_BUDGET).await.unwrap(), 40);
        assert_eq!(big.scan_titles("", 7, DEFAULT_BUDGET).await.unwrap(), 300);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_hang_is_stopped_at_its_deadline() {
        let (rt, loaded, catalog) = setup().await;
        let mut inst = rt.instantiate(&loaded, scope(&catalog, BIG), Opts::default()).await.unwrap();
        let t = Instant::now();
        let err = inst.spin(Duration::from_millis(20)).await.unwrap_err();
        assert!(is_deadline(&err), "{err:?}");
        assert!(t.elapsed() < Duration::from_millis(200));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_crash_is_contained() {
        let (rt, loaded, catalog) = setup().await;
        let mut inst = rt.instantiate(&loaded, scope(&catalog, BIG), Opts::default()).await.unwrap();
        let err = inst.crash().await.unwrap_err();
        assert!(!is_deadline(&err));
        let mut fresh = rt.instantiate(&loaded, scope(&catalog, BIG), Opts::default()).await.unwrap();
        fresh.noop().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn memory_is_limited() {
        let (rt, loaded, catalog) = setup().await;
        let opts = Opts { memory_limit: 32 << 20, ..Opts::default() };
        let mut inst = rt.instantiate(&loaded, scope(&catalog, BIG), opts).await.unwrap();
        assert!(inst.hog(256).await.is_err());
        let opts = Opts { memory_limit: 32 << 20, ..Opts::default() };
        let mut ok = rt.instantiate(&loaded, scope(&catalog, BIG), opts).await.unwrap();
        assert_eq!(ok.hog(8).await.unwrap(), 8);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_paused_plugin_stops_reading() {
        let (rt, loaded, catalog) = setup().await;
        let pause = Arc::new(Pause::default());
        let s = scope(&catalog, BIG);
        let probe = s.probe.clone();
        let mut inst = rt.instantiate(&loaded, s, Opts { pause: pause.clone(), ..Opts::default() }).await.unwrap();
        // Paused only once it exists: instantiating runs plugin code too, and a tick landing
        // there would park the instantiation itself, which this test never resumes.
        pause.set(true);
        let task = tokio::spawn(async move { inst.scan_titles("", 1, DEFAULT_BUDGET).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        // Paused before the scan starts: at most the calls before the first tick got through.
        let seen = probe.calls.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(probe.calls.load(Ordering::Relaxed), seen);
        pause.set(false);
        assert_eq!(task.await.unwrap().unwrap(), 300);
    }
}
