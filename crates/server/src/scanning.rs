//! Background work on every library: its scanner and the triggers that queue scans, loudness
//! and waveform analysis, and image placeholders (`design/scanning.md`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use jewelcase_ffmpeg::Ffmpeg;
use jewelcase_scanner::{
    Analyzer, Placeholders, Pool, Scanner, ScannerThread, Trigger, Worker, scan_every, watch,
};
use tracing::{info, warn};

use crate::db::{Database, SqliteStore, libraries};

/// Everything running for every library, until [`Scanning::stop`].
pub struct Scanning {
    libraries: Vec<Library>,
    /// The threads that run every library's analysis and placeholders.
    background: Pool,
}

/// One library's scanner and everything running for it. Fields drop in the order declared, so
/// they are declared in the order [`Scanning::stop`] stops them, and a library dropped whole, as
/// when [`Scanning::start`] fails partway, stops the same way.
struct Library {
    id: i64,
    scanner: Scanner,
    watcher: Option<Worker>,
    schedule: Option<Worker>,
    thread: ScannerThread,
}

impl Scanning {
    /// Starts a scanner and its triggers for every library, and one pool for every library's
    /// background jobs, sized from the host's cores. Blocks, so call it outside the async
    /// runtime. Fails if the store or a thread cannot be started.
    pub fn start(db: &Arc<Database>, ffmpeg: &Ffmpeg) -> anyhow::Result<Scanning> {
        let store = Arc::new(SqliteStore::new(db.clone()).context("cannot initialise the store")?);

        let libs = db.read_blocking(libraries::all)?;
        if libs.is_empty() {
            warn!("no libraries configured; set JEWELCASE_MUSIC to create one on first start");
        }
        let mut running = Vec::with_capacity(libs.len());
        let mut jobs = Vec::with_capacity(2 * libs.len());
        for lib in &libs {
            let config = lib.config();
            let (scanner, thread) = Scanner::start(store.clone(), config.clone())?;
            scanner.scan_library(startup_trigger(db, lib.id)?);
            let watcher =
                lib.watch.then(|| watch(scanner.clone(), &config.roots, Duration::from_secs(2)));
            let schedule = lib.scan_interval_minutes.map(|minutes| {
                scan_every(scanner.clone(), Duration::from_mins(minutes.unsigned_abs()))
            });
            info!(library = lib.id, name = %lib.name, roots = ?config.roots, "scanner started");
            running.push(Library {
                id: lib.id,
                scanner,
                watcher: watcher.transpose()?,
                schedule: schedule.transpose()?,
                thread,
            });
            // One of each per library, since each remembers its library's failures.
            jobs.push(Arc::new(Analyzer::new(ffmpeg.clone(), store.clone())).job(config.id));
            jobs.push(Arc::new(Placeholders::new(ffmpeg.clone(), store.clone())).job(config.id));
        }
        let background = Pool::start(jobs, Pool::size_for_host())?;
        Ok(Scanning { libraries: running, background })
    }

    /// Each library's scanner, for queuing scans from elsewhere, such as of what a plugin saved.
    pub fn scanners(&self) -> HashMap<i64, Scanner> {
        self.libraries.iter().map(|lib| (lib.id, lib.scanner.clone())).collect()
    }

    /// Stops everything at once: asks every scanner to stop after the file in hand before waiting
    /// for any, so the libraries stop together rather than one after another. Then it waits for
    /// the background jobs, which give up between reads and so finish within a probe or an
    /// image decode, and for each library's triggers and scanner. Blocks, so call it outside the
    /// async runtime.
    pub fn stop(self) {
        for lib in &self.libraries {
            lib.thread.stop();
        }
        drop(self.background);
        for lib in self.libraries {
            // Dropped one by one, not whole, so the order they stop in reads here and does not
            // rest on `Library`'s declaration alone.
            drop(lib.watcher);
            drop(lib.schedule);
            drop(lib.thread);
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
            let (temp, db) = testing::open();
            let (music, other) = (temp.path().join("music"), temp.path().join("other"));
            fs::create_dir(&music).unwrap();
            fs::create_dir(&other).unwrap();
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

            let scanning =
                Scanning::start(&db, &Ffmpeg::new(FfmpegConfig::default()).unwrap()).unwrap();
            let trigger = db.read_blocking(|conn| {
                let startup = "SELECT trigger FROM scans WHERE library_id = 1 AND id != 1";
                conn.query_row(startup, [], |r| r.get::<_, String>(0))
            });
            scanning.stop();
            assert_eq!(trigger.unwrap(), expected, "after a scan {earlier:?}");
        }
    }
}
