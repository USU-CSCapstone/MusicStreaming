//! Background work on every library: its scanner and the triggers that queue scans, loudness
//! and waveform analysis, and image placeholders (`design/scanning.md`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use jewelcase_ffmpeg::Ffmpeg;
use jewelcase_scanner::analysis::{AnalysisWorker, Analyzer};
use jewelcase_scanner::placeholders::{PlaceholderWorker, Placeholders};
use jewelcase_scanner::scan::ScanOptions;
use jewelcase_scanner::triggers::{FsWatcher, Schedule};
use jewelcase_scanner::{Governor, Scanner, Trigger};
use tracing::{info, warn};

use crate::db::{Database, SqliteStore, libraries};

/// Everything running for every library, until [`Scanning::stop`].
pub struct Scanning {
    governor: Arc<Governor>,
    libraries: Vec<Library>,
}

struct Library {
    id: i64,
    scanner: Scanner,
    watcher: Option<FsWatcher>,
    schedule: Option<Schedule>,
    analysis: AnalysisWorker,
    placeholders: PlaceholderWorker,
}

impl Scanning {
    /// Starts a scanner, its triggers, and the background jobs for every library. Blocks, so
    /// call it outside the async runtime.
    pub fn start(db: &Arc<Database>, ffmpeg: &Ffmpeg) -> anyhow::Result<Scanning> {
        let store = Arc::new(SqliteStore::new(db.clone()).context("cannot initialise the store")?);
        let governor = Arc::new(Governor::new());
        let analyzer = Arc::new(Analyzer::new(ffmpeg.clone(), store.clone(), governor.clone()));
        let placeholders = Arc::new(Placeholders::new(ffmpeg.clone(), store.clone()));

        let libs = db.read_blocking(libraries::all)?;
        if libs.is_empty() {
            warn!("no libraries configured; set JEWELCASE_MUSIC to create one on first start");
        }
        let mut running = Vec::with_capacity(libs.len());
        for lib in &libs {
            let config = lib.config();
            let scanner = Scanner::start(
                store.clone(),
                config.clone(),
                governor.clone(),
                ScanOptions::default(),
            );
            scanner.scan_library(Trigger::Initial);
            let watcher = lib
                .watch
                .then(|| FsWatcher::start(scanner.clone(), &config.roots, Duration::from_secs(2)))
                .and_then(|started| {
                    started
                        .map_err(
                            |e| warn!(error = %e, "watcher unavailable; scheduled scans cover it"),
                        )
                        .ok()
                });
            let schedule = lib.scan_interval_minutes.map(|m| {
                Schedule::start(scanner.clone(), Duration::from_secs(m.max(1) as u64 * 60))
            });
            info!(library = lib.id, name = %lib.name, roots = ?config.roots, "scanner started");
            running.push(Library {
                id: lib.id,
                scanner,
                watcher,
                schedule,
                analysis: analyzer.clone().start(config.id.clone(), Duration::from_secs(30)),
                // Soon after a scan finds an image, since clients draw its placeholder at once.
                placeholders: placeholders.clone().start(config.id, Duration::from_secs(5)),
            });
        }
        Ok(Scanning { governor, libraries: running })
    }

    /// Each library's scanner, for queuing scans from elsewhere, such as of what a plugin saved.
    pub fn scanners(&self) -> HashMap<i64, Scanner> {
        self.libraries.iter().map(|lib| (lib.id, lib.scanner.clone())).collect()
    }

    /// Stops everything, each once the work in hand is done: the triggers first, so nothing new
    /// is queued. Blocks, so call it outside the async runtime.
    pub fn stop(self) {
        // A decode the governor has paused would never finish.
        self.governor.resume();
        for lib in self.libraries {
            if let Some(watcher) = lib.watcher {
                watcher.stop();
            }
            if let Some(schedule) = lib.schedule {
                schedule.stop();
            }
            lib.scanner.shutdown();
            lib.analysis.stop();
            lib.placeholders.stop();
        }
        info!("background work stopped");
    }
}
