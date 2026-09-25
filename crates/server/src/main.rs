//! The Jewelcase server. For now: open the database, start the scanner for
//! every library, and answer a health check. The public API follows.

mod db;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::{Json, Router, routing::get};
use jewelcase_ffmpeg::{Config as FfmpegConfig, Ffmpeg};
use jewelcase_scanner::analysis::Analyzer;
use jewelcase_scanner::scan::ScanOptions;
use jewelcase_scanner::triggers::{FsWatcher, Schedule};
use jewelcase_scanner::{Governor, Scanner, Trigger};

use db::{Db, SqliteStore, libraries};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let data = PathBuf::from(std::env::var("JEWELCASE_DATA").unwrap_or_else(|_| "data".into()));
    let port: u16 = std::env::var("JEWELCASE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let bootstrap_music = std::env::var("JEWELCASE_MUSIC").ok().map(PathBuf::from);

    let db = match Db::open(&data.join("state/jewelcase.db")) {
        Ok(db) => Arc::new(db),
        Err(e) => {
            eprintln!("cannot open database: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = std::fs::create_dir_all(data.join("cache")) {
        eprintln!("cannot create cache directory: {e}");
        std::process::exit(1);
    }

    // First start with a music path and no libraries: create one
    // (`requirements/deployment.md` §2).
    if let Some(music) = &bootstrap_music {
        let conn = db.writer();
        match libraries::all(&conn) {
            Ok(libs) if libs.is_empty() => {
                let music = canonical(music);
                match libraries::create(&conn, "Music", &[&music], &[]) {
                    Ok(id) => {
                        tracing::info!(library = id, path = %music.display(), "created library")
                    }
                    Err(e) => tracing::error!(error = %e, "could not create library"),
                }
            }
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "could not list libraries"),
        }
    }

    let ffmpeg = Ffmpeg::new(FfmpegConfig::default());
    match ffmpeg.verify() {
        Ok(caps) if caps.missing.is_empty() => tracing::info!(version = caps.version, "ffmpeg ok"),
        Ok(caps) => {
            tracing::warn!(version = caps.version, missing = ?caps.missing, "ffmpeg lacks decoders; those formats will not be analyzed")
        }
        Err(e) => {
            eprintln!("ffmpeg is required and could not be run: {e}");
            std::process::exit(1);
        }
    }

    let store = match SqliteStore::new(db.clone()) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("cannot initialise store: {e}");
            std::process::exit(1);
        }
    };
    let governor = Arc::new(Governor::new());

    // Held for the life of the process; dropping them would stop the watchers.
    let mut running: Vec<(Scanner, Option<FsWatcher>, Option<Schedule>)> = Vec::new();
    let libs = libraries::all(&db.writer()).unwrap_or_default();
    if libs.is_empty() {
        tracing::warn!("no libraries configured; set JEWELCASE_MUSIC to create one on first start");
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

    // Planner statistics, hourly (`design/database.md` §4).
    {
        let db = db.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(3600));
                if let Err(e) = db.optimize() {
                    tracing::warn!(error = %e, "PRAGMA optimize failed");
                }
            }
        });
    }

    let app = Router::new().route("/health", get(health));
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(%addr, "listening");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
    drop(running);
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
