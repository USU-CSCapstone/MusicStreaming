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

use crate::manifest::Permission;
use crate::rules;

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "lyrics-plugin",
        imports: { default: async },
        exports: { default: async },
    });
}

use bindings::jewelcase::plugin::{host, http, library};
use bindings::{LyricsPlugin, LyricsPluginPre};

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

/// The one library a run is for, as the host reads it. Paths stay with the host.
pub trait Library: Send + 'static {
    /// A page of tracks in a stable order; empty past the end.
    fn tracks(
        &mut self,
        offset: u32,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Track>, String>> + Send;

    /// Where `track`'s audio is, if it is in this library and has no lyrics yet.
    fn audio_without_lyrics(
        &mut self,
        track: u64,
    ) -> impl Future<Output = Result<PathBuf, String>> + Send;
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
    /// Files it saved into the library, for the scanner to pick up.
    pub saved: Vec<PathBuf>,
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
                return Outcome { ok: false, summary, log: Vec::new(), saved: Vec::new() };
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
            log: Vec::new(),
            saved: Vec::new(),
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
        Outcome { ok, summary, log: run.log, saved: run.saved }
    }

    fn pre<L: Library>(&self, component: &Component) -> Result<LyricsPluginPre<Run<L>>, String> {
        let mut linker = Linker::new(&self.engine);
        // WASI with nothing granted: no files, sockets, or environment. Only the imports below.
        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|e| format!("{e:?}"))?;
        LyricsPlugin::add_to_linker::<Run<L>, HasSelf<Run<L>>>(&mut linker, |run| run)
            .map_err(|e| format!("{e:?}"))?;
        let pre = linker.instantiate_pre(component).map_err(|e| e.root_cause().to_string())?;
        LyricsPluginPre::new(pre).map_err(|e| e.root_cause().to_string())
    }
}

/// Instantiates the plugin and calls its `run`, within the wall-clock budget.
async fn call<L: Library>(
    pre: &LyricsPluginPre<Run<L>>,
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

    async fn audio_without_lyrics(&mut self, _: u64) -> Result<PathBuf, String> {
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
    log: Vec<String>,
    saved: Vec<PathBuf>,
}

impl<L: Library> Run<L> {
    fn may(&self, permission: Permission) -> Result<(), String> {
        if self.grants.permissions.contains(&permission) {
            Ok(())
        } else {
            Err(format!("{} was not granted", permission.title()))
        }
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
        Permission::LibraryWrite => host::Permission::LibraryWrite,
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

    async fn save_lyrics(&mut self, track: u64, synced: bool, text: String) -> Result<(), String> {
        self.may(Permission::LibraryWrite)?;
        let audio = self.library.audio_without_lyrics(track).await?;
        let saved = rules::save_lyrics(&audio, synced, &text)?;
        self.saved.push(saved);
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
