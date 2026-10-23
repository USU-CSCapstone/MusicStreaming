mod api;
pub mod config;
mod data_dir;
mod db;
mod lock;
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
use tokio::signal::unix::{SignalKind, signal};
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
    let ffmpeg = Ffmpeg::new(ffmpeg_config.clone()).context("could not start ffmpeg's watchdog")?;
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

/// Waits for SIGINT or SIGTERM. It listens for them only once first polled, so until the server
/// serves, either signal ends the process as it would any other. If it can't listen, it logs
/// that and returns at once, so the server stops rather than run with no way to stop it cleanly.
async fn shutdown_signal() {
    match (signal(SignalKind::interrupt()), signal(SignalKind::terminate())) {
        (Ok(mut interrupt), Ok(mut terminate)) => {
            tokio::select! {
                _ = interrupt.recv() => {}
                _ = terminate.recv() => {}
            }
        }
        (Err(error), _) | (_, Err(error)) => error!(%error, "cannot listen for SIGINT and SIGTERM"),
    }
    info!("shutting down");
}

#[cfg(test)]
mod tests;
