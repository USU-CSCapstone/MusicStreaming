mod api;
pub mod config;
mod data_dir;
mod db;
mod scanning;
mod web;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use jewelcase_ffmpeg::{Config as FfmpegConfig, Ffmpeg};
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::config::Config;
use crate::data_dir::DataDir;
use crate::db::{Database, libraries};
use crate::scanning::Scanning;

/// Runs the server until shutdown_signal resolves, then finishes in-flight requests.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let data_dir = DataDir::open(&config.data_dir)
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
    let mut app = api::router(&config.base_path, db.clone(), images);
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
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server failed")?;
    tokio::task::spawn_blocking(move || scanning.stop()).await?;

    info!("stopped");
    Ok(())
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
