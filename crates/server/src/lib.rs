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
use tracing::{info, warn};

use crate::config::Config;
use crate::data_dir::{DataDir, apart};
use crate::db::{Database, libraries};
use crate::plugins::Plugins;
use crate::scanning::Scanning;

/// Runs the server until shutdown_signal resolves, then finishes in-flight requests.
pub async fn run(config: Config) -> anyhow::Result<()> {
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

    let ffmpeg = Ffmpeg::new(FfmpegConfig::default());
    let caps = ffmpeg.verify().context("ffmpeg is required and could not be run")?;
    if caps.missing.is_empty() {
        info!(version = caps.version, "ffmpeg ok");
    } else {
        warn!(version = caps.version, missing = ?caps.missing, "ffmpeg lacks decoders; those formats will not be analyzed");
    }

    // The scanner calls the database with blocking calls, so it starts outside the async runtime.
    let scanning = {
        let db = db.clone();
        let ffmpeg = ffmpeg.clone();
        tokio::task::spawn_blocking(move || Scanning::start(&db, &ffmpeg)).await??
    };

    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.port));
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot listen on port {}", config.port))?;
    info!(%address, "listening");

    let images = api::Images::new(data_dir.cache().join("images"), ffmpeg.config().ffmpeg.clone());
    let plugins =
        Arc::new(Plugins::new(db.clone(), data_dir.state().join("plugins"), scanning.scanners()));
    plugins.start_hooks();
    let players = Arc::new(api::Players::load(&db).await?);
    let saving = players.keep_saved(db.clone());
    let mut app = api::router(&config.base_path, db.clone(), images, plugins, players.clone());
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
    serve::serve(listener, app, shutdown_signal()).await;
    // Where everyone was, as the server stops: not just as of the last periodic save.
    saving.abort();
    let _ = saving.await;
    players.save(&db).await;
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
/// (`requirements/deployment.md` §2).
async fn create_first_library(db: &Database, music: &Path) -> anyhow::Result<()> {
    if !db.read(libraries::all).await?.is_empty() {
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

    use tokio::time::timeout;

    use super::*;
    use crate::db::testing;

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
    async fn a_first_start_needs_the_music_there() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(&temp.path().join("data"), None).unwrap();
        let db = Database::open(&data_dir.state().join("jewelcase.db")).unwrap();
        let error = create_first_library(&db, &temp.path().join("unmounted")).await.unwrap_err();
        assert!(format!("{error:#}").contains(config::MUSIC), "{error:#}");
    }
}
