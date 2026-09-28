pub mod config;
pub mod data_dir;
pub mod db;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::{Json, Router, routing::get};
use jewelcase_ffmpeg::{Config as FfmpegConfig, Ffmpeg};
use jewelcase_scanner::analysis::Analyzer;
use jewelcase_scanner::scan::ScanOptions;
use jewelcase_scanner::triggers::{FsWatcher, Schedule};
use jewelcase_scanner::{Governor, Scanner, Trigger};
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::config::Config;
use crate::data_dir::DataDir;
use crate::db::{Database, SqliteStore, libraries};

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
    let caps = ffmpeg
        .verify()
        .context("ffmpeg is required and could not be run")?;
    if caps.missing.is_empty() {
        info!(version = caps.version, "ffmpeg ok");
    } else {
        warn!(version = caps.version, missing = ?caps.missing, "ffmpeg lacks decoders; those formats will not be analyzed");
    }

    // Held until shutdown; dropping them stops the watchers. The scanner calls the database
    // with blocking calls, so it starts outside the async runtime.
    let _scanning = {
        let db = db.clone();
        let ffmpeg = ffmpeg.clone();
        tokio::task::spawn_blocking(move || start_scanning(&db, &ffmpeg)).await??
    };

    // Planner statistics, hourly (`design/database.md` §4).
    {
        let db = db.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(3600));
                if let Err(e) = db.write_blocking(|tx| tx.execute_batch("PRAGMA optimize")) {
                    tracing::warn!(error = %e, "PRAGMA optimize failed");
                }
            }
        });
    }

    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.port));
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot listen on port {}", config.port))?;
    info!(%address, "listening");

    let app = Router::new().route("/health", get(health));
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server failed")?;

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
    let id = db
        .write(move |tx| libraries::create(tx, None, "Music", &[&path], &[]))
        .await?;
    info!(library = id, path = %music.display(), "created library");
    Ok(())
}

/// Each library's scanner, and its watcher and schedule if it has them.
type Scanning = Vec<(Scanner, Option<FsWatcher>, Option<Schedule>)>;

/// Starts a scanner, its triggers, and an analyzer for every library. Blocks, so call it
/// outside the async runtime.
fn start_scanning(db: &Arc<Database>, ffmpeg: &Ffmpeg) -> anyhow::Result<Scanning> {
    let store = Arc::new(SqliteStore::new(db.clone()).context("cannot initialise the store")?);
    let governor = Arc::new(Governor::new());

    let mut running = Vec::new();
    let libs = db.read_blocking(libraries::all)?;
    if libs.is_empty() {
        warn!("no libraries configured; set JEWELCASE_MUSIC to create one on first start");
    }
    for lib in &libs {
        let config = lib.config();
        let scanner = Scanner::start(
            store.clone(),
            config.clone(),
            governor.clone(),
            ScanOptions::default(),
        );
        scanner.scan_library(Trigger::Initial);
        let watcher = if lib.watch {
            FsWatcher::start(scanner.clone(), &config.roots, Duration::from_secs(2))
                .map_err(
                    |e| tracing::warn!(error = %e, "watcher unavailable; scheduled scans cover it"),
                )
                .ok()
        } else {
            None
        };
        let schedule = lib
            .scan_interval_minutes
            .map(|m| Schedule::start(scanner.clone(), Duration::from_secs(m.max(1) as u64 * 60)));
        tracing::info!(library = lib.id, name = %lib.name, roots = ?config.roots, "scanner started");
        running.push((scanner, watcher, schedule));

        let analyzer = Arc::new(Analyzer::new(
            ffmpeg.clone(),
            store.clone(),
            governor.clone(),
        ));
        let _worker = analyzer.start(config.id.clone(), Duration::from_secs(30));
        std::mem::forget(_worker);
    }
    Ok(running)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Wait for SIGINT or SIGTERM and return
async fn shutdown_signal() {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .expect("cannot listen for SIGINT");
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
