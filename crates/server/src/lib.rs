mod api;
pub mod config;
mod data_dir;
mod db;
mod plugins;
mod scanning;
mod serve;
mod web;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use jewelcase_ffmpeg::{Config as FfmpegConfig, Ffmpeg};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::data_dir::{DataDir, apart};
use crate::db::{Database, libraries};
use crate::plugins::Plugins;
use crate::scanning::Scanning;

/// Runs the server until SIGINT or SIGTERM, then stops it as [`run_until`] does.
pub async fn run(config: Config) -> anyhow::Result<()> {
    run_until(config, shutdown_signal()).await
}

/// Runs the server until `shutdown` resolves, then stops it in the order that loses least if a
/// kill cuts the stop short, as `docker stop` does after 10 s: it stops taking connections,
/// closes the realtime ones and saves where everyone was playing while it waits a few seconds
/// at most for the requests in progress, and last it stops the libraries' background work.
async fn run_until(config: Config, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    let music = config.music.as_deref().and_then(resolve_music);
    let data_dir = DataDir::open(&config.data_dir, music.as_deref())
        .with_context(|| format!("cannot use the data directory set by {}", config::DATA_DIR))?;
    info!(
        data_dir = %data_dir.root().display(),
        port = config.port,
        base_path = if config.base_path.is_empty() { "/" } else { &config.base_path },
        "starting"
    );

    // Run migrations before listening for connections
    let db = Arc::new(Database::open(&data_dir.state().join("jewelcase.db"))?);
    if let Some(music) = &config.music {
        create_first_library(&db, music).await?;
    }
    roots_apart(&db, &data_dir).await?;

    let ffmpeg_config = FfmpegConfig::default();
    let ffmpeg = Ffmpeg::new(ffmpeg_config.clone());
    let caps = ffmpeg.verify().context("ffmpeg is required and could not be run")?;
    if caps.missing.is_empty() {
        info!(version = caps.version, "ffmpeg ok");
    } else {
        warn!(version = caps.version, missing = ?caps.missing, "ffmpeg lacks decoders; those formats will not be analyzed");
    }

    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.port));
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot listen on port {}", config.port))?;
    info!(%address, "listening");

    // The scanner calls the database with blocking calls, so it starts outside the async runtime.
    let scanning = {
        let db = db.clone();
        let started = tokio::task::spawn_blocking(move || Scanning::start(&db, &ffmpeg)).await?;
        started.context("cannot start the libraries' background work")?
    };

    let images = api::Images::new(data_dir.cache().join("images"), ffmpeg_config);
    // A sweep that fails costs only disk until the next start, so the server starts anyway.
    if let Err(error) = images.sweep(&db).await {
        warn!(error = %format!("{error:#}"), "cannot sweep the image cache");
    }
    // A cache folder that fails the check costs only a compile per plugin after each restart,
    // so the server starts without it rather than refusing to.
    let compiled = match data_dir.private_cache("plugins") {
        Ok(dir) => Some(dir),
        Err(error) => {
            warn!(error = %format!("{error:#}"), "plugins will be compiled again after each restart");
            None
        }
    };
    let installed = data_dir.state().join("plugins");
    let plugins = Arc::new(Plugins::new(db.clone(), installed, compiled, scanning.scanners()));
    plugins.start_hooks();
    let players = Arc::new(api::Players::load(&db).await?);
    let saving = players.keep_saved(db.clone());
    let indexes = Arc::new(api::Indexes::default());
    indexes.build_all(&db);
    let hub = Arc::new(api::Hub::default());
    let mut app = api::router(
        &config.base_path,
        db.clone(),
        images,
        indexes,
        plugins,
        players.clone(),
        hub.clone(),
    );
    let web = Path::new(web::DIR);
    if !web.is_dir() {
        info!(dir = web::DIR, "no web app there; serving the API only");
    } else if !config.base_path.is_empty() {
        // The build's links assume the root until startup rewrites them for the base path
        // (`design/general.md` §7.1).
        warn!("the web app cannot be served under a base path yet; serving the API only");
    } else {
        app = app.merge(web::router(web));
    }
    // Run once the server accepts no more connections. A closed realtime connection acts for no
    // one, and a report it was checking is dropped, so after the save only a connection whose
    // upgrade was already under way could still report.
    let as_it_stops = async {
        hub.close_all();
        // The periodic save stops first, leaving a write already begun to finish.
        saving.abort();
        if let Err(error) = saving.await
            && error.is_panic()
        {
            error!(%error, "the periodic save of playback state panicked");
        }
        players.save(&db).await;
    };
    serve::serve(listener, app, shutdown, as_it_stops).await;
    tokio::task::spawn_blocking(move || scanning.stop()).await?;

    info!("stopped");
    Ok(())
}

/// Canonicalizes `JEWELCASE_MUSIC` so it can be checked against the data directory before
/// anything is written there. Returns `None` with a warning when it cannot be resolved: storage
/// not mounted yet stops nothing once libraries exist (`requirements/libraries.md` §2), and on a
/// first start [`create_first_library`] fails instead.
fn resolve_music(music: &Path) -> Option<PathBuf> {
    match music.canonicalize() {
        Ok(music) => Some(music),
        Err(error) => {
            warn!(%error, setting = config::MUSIC, "cannot resolve the music directory");
            None
        }
    }
}

/// Creates a library from `JEWELCASE_MUSIC` on first start, when there are none yet
/// (`requirements/deployment.md` §2). Once there are, the setting changes nothing, so it warns
/// when the music it names is no library's root.
async fn create_first_library(db: &Database, music: &Path) -> anyhow::Result<()> {
    let libraries = db.read(libraries::all).await?;
    if !libraries.is_empty() {
        // Music that cannot be resolved has been warned of already (`resolve_music`).
        if let Ok(music) = music.canonicalize()
            && !libraries.iter().flat_map(|library| &library.roots).any(|root| root.path == music)
        {
            warn!(
                setting = config::MUSIC,
                music = %music.display(),
                "no library has this music as a root; the setting only creates the first library"
            );
        }
        return Ok(());
    }
    let music = music
        .canonicalize()
        .with_context(|| format!("cannot use the music directory set by {}", config::MUSIC))?;
    let path = music.clone();
    let id = db.write(move |tx| libraries::create(tx, None, "Music", &[&path], &[])).await?;
    info!(library = id, path = %music.display(), "created library");
    Ok(())
}

/// Fails when any library's root and the data directory overlap.
async fn roots_apart(db: &Database, data_dir: &DataDir) -> anyhow::Result<()> {
    for library in db.read(libraries::all).await? {
        for root in &library.roots {
            apart(data_dir.root(), &root.path)?;
        }
    }
    Ok(())
}

/// Wait for SIGINT or SIGTERM and return
async fn shutdown_signal() {
    let interrupt = async {
        tokio::signal::ctrl_c().await.expect("cannot listen for SIGINT");
    };
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("cannot listen for SIGTERM")
            .recv()
            .await;
    };
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    info!("shutting down");
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use futures_util::StreamExt;
    use rusqlite::OptionalExtension;
    use serde_json::json;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;
    use tokio::time::{Instant, sleep, timeout};
    use tokio_tungstenite::tungstenite::Message;

    use super::*;
    use crate::api::testing::player::{report, start_playing};
    use crate::api::testing::socket::connect_with;
    use crate::api::testing::{TOKEN, add_device};
    use crate::db::testing;
    use crate::serve::DRAIN_TIMEOUT;

    /// How long `run` gets to refuse: a server that does not stop has started serving.
    const REFUSED: Duration = Duration::from_secs(10);

    /// A server configured with `data_dir` and `music`, on any free port.
    fn config(data_dir: &Path, music: Option<&Path>) -> Config {
        let music = music.map(Path::to_owned);
        Config { data_dir: data_dir.to_owned(), music, port: 0, ..Config::default() }
    }

    #[tokio::test]
    async fn a_data_directory_inside_the_music_stops_the_server_having_written_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let music = temp.path().canonicalize().unwrap();
        let refused = timeout(REFUSED, run(config(&music.join("data"), Some(&music)))).await;
        let error = refused.unwrap().unwrap_err();
        assert!(format!("{error:#}").contains("overlap"), "{error:#}");
        assert_eq!(fs::read_dir(music.join("data")).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_library_root_holding_the_data_directory_stops_the_server() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        {
            let data_dir = DataDir::open(&data, None).unwrap();
            let db = Database::open(&data_dir.state().join("jewelcase.db")).unwrap();
            let music = temp.path().canonicalize().unwrap();
            db.write(move |tx| testing::library(tx, 1, "Music", &music)).await.unwrap();
        }
        // Music not mounted yet is passed over, so the server still gets to the stored roots.
        let unmounted = temp.path().join("unmounted");
        for music in [None, Some(unmounted.as_path())] {
            let error = timeout(REFUSED, run(config(&data, music))).await.unwrap().unwrap_err();
            assert!(format!("{error:#}").contains("overlap"), "{music:?}: {error:#}");
        }
    }

    #[test]
    fn music_not_there_yet_is_not_checked() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(resolve_music(temp.path()), Some(temp.path().canonicalize().unwrap()));
        assert_eq!(resolve_music(&temp.path().join("unmounted")), None);
    }

    #[tokio::test]
    async fn music_that_is_no_librarys_root_is_warned_of() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(&temp.path().join("data"), None).unwrap();
        let db = Database::open(&data_dir.state().join("jewelcase.db")).unwrap();
        let (music, other) = (temp.path().join("music"), temp.path().join("other"));
        fs::create_dir(&music).unwrap();
        fs::create_dir(&other).unwrap();
        create_first_library(&db, &music).await.unwrap();

        let log = temp.path().join("log");
        let writer = fs::File::create(&log).unwrap();
        let _logging = tracing::subscriber::set_default(
            tracing_subscriber::fmt().with_writer(writer).with_ansi(false).finish(),
        );
        // The library's own root, written another way.
        create_first_library(&db, &temp.path().join("other/../music")).await.unwrap();
        assert_eq!(fs::read_to_string(&log).unwrap(), "", "the library's own root");
        create_first_library(&db, &other).await.unwrap();
        let logged = fs::read_to_string(&log).unwrap();
        assert!(logged.contains("WARN") && logged.contains(config::MUSIC), "{logged}");
        assert_eq!(db.read(libraries::all).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_first_start_needs_the_music_there() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(&temp.path().join("data"), None).unwrap();
        let db = Database::open(&data_dir.state().join("jewelcase.db")).unwrap();
        let error = create_first_library(&db, &temp.path().join("unmounted")).await.unwrap_err();
        assert!(format!("{error:#}").contains(config::MUSIC), "{error:#}");
    }

    /// A copy of an image no library holds is deleted before the server takes requests.
    #[tokio::test]
    async fn the_image_cache_is_swept_as_the_server_starts() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let cache = DataDir::open(&data, None).unwrap().cache().to_owned();
        let unheld = cache.join("images/1/01/01-64.jpg");
        fs::create_dir_all(unheld.parent().unwrap()).unwrap();
        fs::write(&unheld, "").unwrap();
        let ran = timeout(REFUSED, run_until(config(&data, None), std::future::ready(())));
        ran.await.unwrap().unwrap();
        assert!(!unheld.exists(), "a copy of an image no library holds is kept");
    }

    /// As the server stops, it closes each realtime connection with a Close frame and saves where
    /// its account was playing at once, rather than once the requests in progress are answered.
    #[tokio::test]
    async fn a_stop_saves_players_and_closes_connections_while_a_request_hangs() {
        let temp = tempfile::tempdir().unwrap();
        let (data, music) = (temp.path().join("data"), temp.path().join("music"));
        fs::create_dir(&music).unwrap();
        let state = DataDir::open(&data, None).unwrap().state().join("jewelcase.db");
        Database::open(&state)
            .unwrap()
            .write(move |tx| {
                testing::User::new(1, "owner").insert(tx)?;
                add_device(tx, 1, 1, TOKEN);
                testing::library(tx, 1, "Music", &music)?;
                testing::Album::new(101, 1).insert(tx)?;
                testing::Track::new(1, 1, 101).insert(tx)
            })
            .await
            .unwrap();
        // A port free a moment ago, since the test must know it.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let server = run_until(Config { port, ..config(&data, None) }, async {
            stopping.await.unwrap();
        });

        let client = async {
            for _ in 0..100 {
                if TcpStream::connect(address).await.is_ok() {
                    break;
                }
                sleep(Duration::from_millis(100)).await;
            }
            let mut socket = connect_with(address, TOKEN).await;
            start_playing(&mut socket, "1").await;
            report(&mut socket, "1", "playing", json!({ "positionMs": 60_000 })).await;
            // Part of a header holds the drain open, once the server has the connection.
            let mut hanging = TcpStream::connect(address).await.unwrap();
            hanging.write_all(b"GET /api/v1/health HTTP/1.1\r\n").await.unwrap();
            sleep(Duration::from_millis(200)).await;
            stop.send(()).unwrap();
            let stopped = Instant::now();

            loop {
                match timeout(Duration::from_secs(1), socket.next()).await.unwrap() {
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                    other => panic!("closed without a Close frame: {other:?}"),
                }
            }
            let conn = rusqlite::Connection::open(&state).unwrap();
            let saved = "SELECT position_ms FROM player_states WHERE user_id = 1";
            loop {
                let position = conn.query_row(saved, [], |r| r.get::<_, u32>(0)).optional();
                if position.unwrap().is_some_and(|position| position >= 60_000) {
                    break;
                }
                assert!(stopped.elapsed() < DRAIN_TIMEOUT / 2, "not saved before the drain");
                sleep(Duration::from_millis(10)).await;
            }
            hanging
        };
        let (ran, _hanging) = tokio::join!(timeout(DRAIN_TIMEOUT + REFUSED, server), client);
        ran.unwrap().unwrap();
    }
}
