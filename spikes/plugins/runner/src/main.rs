//! Runs an installed plugin once, with exactly the permissions its admin approved.
//!
//!     plugin-run <plugin-id> --data <data dir>
//!
//! Reads the installed plugin and its grants from `<data>/mock-plugins` (written by the web
//! mock's Plugins page) and the library from the scanner's `<data>/state/jewelcase.db`, then
//! calls the plugin's `run` once for each library it is enabled in. Output is JSON lines:
//! `{"log": "..."}` as it goes, then `{"done": {"ok", "summary", "saved"}}`.

mod rules;

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::json;
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable, bindgen};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

bindgen!({
    path: "../wit/lyrics",
    world: "lyrics-plugin",
    imports: { "jewelcase:plugin/http.get": async },
    exports: { default: async },
});

use jewelcase::plugin::host::Permission;
use jewelcase::plugin::{http, library};

/// Compute a plugin may spend in one run, counted while its code runs.
const CPU_BUDGET: Duration = Duration::from_secs(60);
/// Wall-clock time for a whole run. Epoch deadlines stop while the host awaits a network
/// reply, so this is what bounds a plugin waiting on a slow service.
const WALL_BUDGET: Duration = Duration::from_secs(300);
const MEMORY_LIMIT: usize = 64 << 20;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const HTTP_MAX_BODY: usize = 2 << 20;
const USER_AGENT: &str = "Jewelcase/0.1 lrclib-lyrics (plugin prototype)";

fn emit(v: serde_json::Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{v}").ok();
    out.flush().ok();
}

// ───────────────────────────── Installed state ─────────────────────────────

#[derive(Deserialize)]
struct Index {
    plugins: HashMap<String, Installed>,
}

#[derive(Deserialize)]
struct Installed {
    manifest: Manifest,
    granted: Vec<String>,
    libraries: HashMap<String, LibraryEntry>,
}

#[derive(Deserialize)]
struct Manifest {
    name: String,
    permissions: Vec<Request>,
}

#[derive(Deserialize)]
struct Request {
    permission: String,
    required: bool,
    #[serde(default)]
    destinations: Vec<String>,
}

#[derive(Deserialize)]
struct LibraryEntry {
    enabled: bool,
    granted: Vec<String>,
}

fn permission(name: &str) -> Option<Permission> {
    Some(match name {
        "libraryRead" => Permission::LibraryRead,
        "libraryWrite" => Permission::LibraryWrite,
        "network" => Permission::Network,
        "listeningActivity" => Permission::ListeningActivity,
        _ => return None,
    })
}

// ───────────────────────────── The library, host-side ─────────────────────────────

struct TrackRow {
    id: u64,
    title: String,
    artists: Vec<String>,
    album: Option<String>,
    duration_ms: u64,
    has_lyrics: bool,
    /// Never shown to the plugin.
    audio: PathBuf,
}

fn load_tracks(db: &rusqlite::Connection, library: i64) -> Result<Vec<TrackRow>> {
    let mut artists = db.prepare(
        "SELECT a.name FROM track_artists ta JOIN artists a ON a.id = ta.artist_id
         WHERE ta.track_id = ?1 ORDER BY ta.position",
    )?;
    let mut stmt = db.prepare(
        "SELECT t.id, t.title, t.duration_us, t.lyrics_kind, t.path, r.path, al.title
         FROM tracks t JOIN library_roots r ON r.id = t.root_id JOIN albums al ON al.id = t.album_id
         WHERE t.library_id = ?1 AND t.missing_since IS NULL ORDER BY t.sort_key, t.id",
    )?;
    let rows = stmt.query_map([library], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, Option<String>>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, title, duration_us, lyrics, path, root, album) = row?;
        let names = artists
            .query_map([id], |r| r.get::<_, Option<String>>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect();
        out.push(TrackRow {
            id: id as u64,
            title,
            artists: names,
            album,
            duration_ms: (duration_us / 1000) as u64,
            has_lyrics: lyrics != "none",
            audio: Path::new(&root).join(path),
        });
    }
    Ok(out)
}

// ───────────────────────────── Host imports ─────────────────────────────

struct Run {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    deadline: Instant,
    granted: Vec<Permission>,
    destinations: Vec<String>,
    tracks: Vec<TrackRow>,
    client: reqwest::Client,
    saved: u32,
}

impl WasiView for Run {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// Permission names as the admin saw them on the Plugins page.
fn label(p: Permission) -> &'static str {
    match p {
        Permission::LibraryRead => "Read the library",
        Permission::LibraryWrite => "Write to the library",
        Permission::Network => "Network access",
        Permission::ListeningActivity => "Listening activity",
    }
}

impl Run {
    fn may(&self, p: Permission) -> Result<(), String> {
        if self.granted.contains(&p) {
            Ok(())
        } else {
            Err(format!("{} was not granted", label(p)))
        }
    }
}

impl jewelcase::plugin::host::Host for Run {
    fn granted(&mut self) -> Vec<Permission> {
        self.granted.clone()
    }

    fn log(&mut self, message: String) {
        emit(json!({ "log": message.chars().take(500).collect::<String>() }));
    }
}

impl library::Host for Run {
    fn tracks(&mut self, offset: u32, limit: u32) -> Result<Vec<library::Track>, String> {
        self.may(Permission::LibraryRead)?;
        let start = (offset as usize).min(self.tracks.len());
        let end = start
            .saturating_add(limit.min(500) as usize)
            .min(self.tracks.len());
        Ok(self.tracks[start..end]
            .iter()
            .map(|t| library::Track {
                id: t.id,
                title: t.title.clone(),
                artists: t.artists.clone(),
                album: t.album.clone(),
                duration_ms: t.duration_ms,
                has_lyrics: t.has_lyrics,
            })
            .collect())
    }

    fn save_lyrics(&mut self, track: u64, synced: bool, text: String) -> Result<(), String> {
        self.may(Permission::LibraryWrite)?;
        let t = self
            .tracks
            .iter_mut()
            .find(|t| t.id == track)
            .ok_or("no such track in this library")?;
        if t.has_lyrics {
            return Err("this track already has lyrics".into());
        }
        rules::save_lyrics(&t.audio, synced, &text)?;
        t.has_lyrics = true;
        self.saved += 1;
        Ok(())
    }
}

impl http::Host for Run {
    async fn get(&mut self, url: String) -> Result<http::Response, String> {
        self.may(Permission::Network)?;
        rules::allowed(&url, &self.destinations)?;
        let fetch = async {
            let mut res = self
                .client
                .get(&url)
                .send()
                .await
                .map_err(|e| format!("request failed: {}", e.without_url()))?;
            let status = res.status().as_u16();
            let mut body = Vec::new();
            while let Some(chunk) = res
                .chunk()
                .await
                .map_err(|e| format!("reply failed: {e}"))?
            {
                body.extend_from_slice(&chunk);
                if body.len() > HTTP_MAX_BODY {
                    return Err("the reply is over 2 MB".to_owned());
                }
            }
            Ok(http::Response {
                status,
                body: String::from_utf8_lossy(&body).into_owned(),
            })
        };
        tokio::time::timeout(HTTP_TIMEOUT, fetch)
            .await
            .map_err(|_| "the request timed out".to_owned())?
    }
}

// ───────────────────────────── Running ─────────────────────────────

struct Outcome {
    summary: String,
    saved: u32,
}

async fn run_once(
    engine: &Engine,
    pre: &LyricsPluginPre<Run>,
    granted: Vec<Permission>,
    destinations: Vec<String>,
    tracks: Vec<TrackRow>,
) -> Result<Outcome> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .build()?;
    let state = Run {
        wasi: WasiCtx::builder().build(),
        table: ResourceTable::new(),
        limits: StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).build(),
        deadline: Instant::now() + CPU_BUDGET,
        granted,
        destinations,
        tracks,
        client,
        saved: 0,
    };
    let mut store = Store::new(engine, state);
    store.limiter(|s| &mut s.limits);
    store.epoch_deadline_callback(|ctx| {
        if Instant::now() >= ctx.data().deadline {
            return Err(wasmtime::Error::msg(
                "the plugin ran past its compute budget",
            ));
        }
        Ok(UpdateDeadline::Yield(1))
    });
    store.set_epoch_deadline(1);
    let plugin = pre
        .instantiate_async(&mut store)
        .await
        .map_err(|e| anyhow!("{e:?}"))?;
    let result = tokio::time::timeout(WALL_BUDGET, plugin.call_run(&mut store))
        .await
        .map_err(|_| {
            anyhow!(
                "the plugin ran past its {} s time limit",
                WALL_BUDGET.as_secs()
            )
        })?
        .map_err(|e| anyhow!("the plugin crashed: {}", e.root_cause()))?;
    let saved = store.data().saved;
    match result {
        Ok(summary) => Ok(Outcome { summary, saved }),
        Err(e) => bail!("the plugin reported an error: {e}"),
    }
}

async fn main_inner(id: &str, data: &Path) -> Result<Outcome> {
    let index_path = data.join("mock-plugins/index.json");
    let index: Index = serde_json::from_slice(
        &std::fs::read(&index_path).with_context(|| format!("reading {}", index_path.display()))?,
    )?;
    let installed = index
        .plugins
        .get(id)
        .ok_or_else(|| anyhow!("{id} is not installed"))?;
    let enabled: Vec<(&String, &LibraryEntry)> = installed
        .libraries
        .iter()
        .filter(|(_, l)| l.enabled)
        .collect();
    if enabled.is_empty() {
        bail!("{} is not enabled in any library", installed.manifest.name);
    }
    let required: Vec<String> = installed
        .manifest
        .permissions
        .iter()
        .filter(|r| r.required)
        .map(|r| r.permission.clone())
        .collect();
    let destinations = installed
        .manifest
        .permissions
        .iter()
        .find(|r| r.permission == "network")
        .map(|r| r.destinations.clone())
        .unwrap_or_default();

    let mut config = Config::new();
    config.epoch_interruption(true);
    let engine = Engine::new(&config).map_err(|e| anyhow!("{e:?}"))?;
    let weak = engine.weak();
    thread::spawn(move || {
        while let Some(engine) = weak.upgrade() {
            engine.increment_epoch();
            drop(engine);
            thread::sleep(Duration::from_millis(1));
        }
    });
    let component = Component::from_file(&engine, data.join(format!("mock-plugins/{id}.wasm")))
        .map_err(|e| anyhow!("{e:?}"))?;
    let mut linker = Linker::new(&engine);
    // WASI with nothing granted: no files, sockets, or environment. Only the imports above.
    wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(|e| anyhow!("{e:?}"))?;
    LyricsPlugin::add_to_linker::<Run, HasSelf<Run>>(&mut linker, |s| s)
        .map_err(|e| anyhow!("{e:?}"))?;
    let pre = LyricsPluginPre::new(
        linker
            .instantiate_pre(&component)
            .map_err(|e| anyhow!("{e:?}"))?,
    )
    .map_err(|e| anyhow!("{e:?}"))?;

    let db = rusqlite::Connection::open_with_flags(
        data.join("state/jewelcase.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut summaries = Vec::new();
    let mut saved = 0;
    for (library_id, entry) in enabled {
        let granted_names: Vec<String> = installed
            .granted
            .iter()
            .chain(entry.granted.iter())
            .cloned()
            .collect();
        let missing = rules::missing_required(&required, &granted_names);
        if !missing.is_empty() {
            let names: Vec<&str> = missing
                .iter()
                .filter_map(|p| permission(p))
                .map(label)
                .collect();
            bail!("it still needs {}", names.join(" and "));
        }
        let granted = granted_names.iter().filter_map(|p| permission(p)).collect();
        let tracks = load_tracks(&db, library_id.parse()?)?;
        let outcome = run_once(&engine, &pre, granted, destinations.clone(), tracks).await?;
        summaries.push(outcome.summary);
        saved += outcome.saved;
    }
    Ok(Outcome {
        summary: summaries.join(" "),
        saved,
    })
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = args
        .iter()
        .position(|a| a == "--data")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"));
    let Some(id) = args.first().filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: plugin-run <plugin-id> --data <data dir>");
        std::process::exit(2);
    };
    match main_inner(id, &data).await {
        Ok(o) => emit(json!({ "done": { "ok": true, "summary": o.summary, "saved": o.saved } })),
        Err(e) => {
            emit(json!({ "done": { "ok": false, "summary": format!("{e:#}"), "saved": 0 } }));
            std::process::exit(1);
        }
    }
}
