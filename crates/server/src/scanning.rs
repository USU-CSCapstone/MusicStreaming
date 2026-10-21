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

/// How often a library without a schedule of its own is scanned while part of it is not watched.
const FALLBACK_SCHEDULE: Duration = Duration::from_secs(24 * 60 * 60);

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

        let libs = db.read_blocking(libraries::all)?;
        if libs.is_empty() {
            warn!("no libraries configured; set JEWELCASE_MUSIC to create one on first start");
        }
        let mut running = Vec::with_capacity(libs.len());
        for lib in &libs {
            let config = lib.config();
            let fallback = lib.scan_interval_minutes.is_none().then_some(FALLBACK_SCHEDULE);
            let scanner = Scanner::start(
                store.clone(),
                config.clone(),
                governor.clone(),
                ScanOptions::default(),
            );
            scanner.scan_library(startup_trigger(db, lib.id)?);
            let watcher = lib.watch.then(|| {
                FsWatcher::start(scanner.clone(), &config.roots, Duration::from_secs(2), fallback)
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
                // One of each per library, since each remembers its library's failures.
                analysis: Arc::new(Analyzer::new(ffmpeg.clone(), store.clone(), governor.clone()))
                    .start(config.id, Duration::from_secs(30)),
                // Soon after a scan finds an image, since clients draw its placeholder at once.
                placeholders: Arc::new(Placeholders::new(ffmpeg.clone(), store.clone()))
                    .start(config.id, Duration::from_secs(5)),
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

/// The trigger of a library's scan at startup, which catches what changed while the server was
/// down: `Scheduled`, or `Initial` until a scan of the library has completed.
fn startup_trigger(db: &Database, library: i64) -> anyhow::Result<Trigger> {
    let scanned = db.read_blocking(move |conn| libraries::scanned(conn, library))?;
    Ok(if scanned { Trigger::Scheduled } else { Trigger::Initial })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use jewelcase_ffmpeg::Config as FfmpegConfig;
    use rusqlite::params;

    use super::*;
    use crate::db::testing;

    /// A library's startup scan is its first until one of its own scans completes, whatever
    /// else it or another library has run.
    #[test]
    fn the_startup_scan_is_initial_until_a_scan_completes() {
        let cases = [
            (None, "initial"),
            (Some("suspended"), "initial"),
            (Some("cancelled"), "initial"),
            (Some("completed"), "scheduled"),
        ];
        for (earlier, expected) in cases {
            let temp = tempfile::tempdir().unwrap();
            let (music, other) = (temp.path().join("music"), temp.path().join("other"));
            fs::create_dir(&music).unwrap();
            fs::create_dir(&other).unwrap();
            let db = Arc::new(Database::open(&temp.path().join("jewelcase.db")).unwrap());
            db.write_blocking(move |tx| {
                testing::library(tx, 1, "Music", &music)?;
                testing::library(tx, 2, "Other", &other)?;
                let scan = "INSERT INTO scans (id, library_id, trigger, state, scopes, created_at) \
                            VALUES (?1, ?2, 'manual', ?3, '[]', 0)";
                tx.execute(scan, params![2, 2, "completed"])?;
                if let Some(state) = earlier {
                    tx.execute(scan, params![1, 1, state])?;
                }
                Ok(())
            })
            .unwrap();

            let scanning = Scanning::start(&db, &Ffmpeg::new(FfmpegConfig::default())).unwrap();
            let trigger = db.read_blocking(|conn| {
                let startup = "SELECT trigger FROM scans WHERE library_id = 1 AND id != 1";
                conn.query_row(startup, [], |r| r.get::<_, String>(0))
            });
            scanning.stop();
            assert_eq!(trigger.unwrap(), expected, "after a scan {earlier:?}");
        }
    }
}
