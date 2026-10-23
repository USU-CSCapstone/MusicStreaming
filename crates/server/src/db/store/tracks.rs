//! Tracks as the scanner sees them: applying a batch, and whether anything is indexed under a
//! folder.

use std::path::Path;

use jewelcase_scanner::Batch;
use rusqlite::{Result, Transaction, params};

use super::SqliteStore;
use super::problems::{clear_problem, record_problem};
use super::roots::bounds_under;
use super::scans;
use super::upsert::upsert_track;
use crate::db::catalog::{self, Touched};
use crate::db::feed::{self, Entity, Op};
use crate::db::{DbError, now_ms};

/// Writes `batch` in one transaction: its tracks, returned files, tracks gone, files changed but
/// unread, problems, the derived data they touch, and the scan row it carries, with the row's
/// missing count raised by the tracks the batch marked missing that were not already. Returns
/// that number.
pub fn apply(store: &SqliteStore, lib: i64, batch: Batch) -> Result<i64, DbError> {
    let roots = store.roots.clone();
    store.db.write_blocking(move |tx| {
        let now = now_ms();
        let mut touched = Touched::default();
        for record in &batch.upserts {
            upsert_track(&roots, tx, lib, record, now, &mut touched)?;
        }
        let mut marked = 0;
        for &id in &batch.missing {
            let changed = tx
                .prepare_cached(
                    "UPDATE tracks SET missing_since = ?3 \
                     WHERE id = ?1 AND library_id = ?2 AND missing_since IS NULL",
                )?
                .execute(params![id, lib, now])?;
            if changed > 0 {
                track_changed(tx, lib, id, now, &mut touched)?;
                marked += 1;
            }
        }
        for &id in &batch.returned {
            let changed = tx
                .prepare_cached(
                    "UPDATE tracks SET missing_since = NULL \
                     WHERE id = ?1 AND library_id = ?2 AND missing_since IS NOT NULL",
                )?
                .execute(params![id, lib])?;
            if changed > 0 {
                track_changed(tx, lib, id, now, &mut touched)?;
            }
        }
        for &id in &batch.changed_unread {
            forget_stamp(tx, lib, id, now, &mut touched)?;
        }
        for path in &batch.cleared {
            clear_problem(&roots, tx, lib, path)?;
        }
        for problem in &batch.problems {
            record_problem(&roots, tx, lib, problem)?;
        }
        catalog::recompute(tx, lib, &touched, now)?;
        if let Some(mut scan) = batch.scan {
            scan.progress.missing += marked;
            scans::save(tx, &roots, &scan)?;
        }
        Ok(marked)
    })
}

/// Applies [`Batch::changed_unread`] to track `id` of `lib`: forgets its mtime and resets its
/// analysis to version 0 with no loudness, peak or waveform. A track this changes is recorded in
/// the feed at `now` and added to `touched`, so its album's loudness follows; one already
/// forgotten, or another library's, is left alone.
fn forget_stamp(
    tx: &Transaction<'_>,
    lib: i64,
    id: i64,
    now: i64,
    touched: &mut Touched,
) -> Result<()> {
    let changed = tx
        .prepare_cached(
            "UPDATE tracks SET file_mtime = NULL, analyzer_version = 0, loudness_lufs = NULL, \
             peak_dbtp = NULL WHERE id = ?1 AND library_id = ?2 AND file_mtime IS NOT NULL",
        )?
        .execute(params![id, lib])?;
    if changed == 0 {
        return Ok(());
    }
    tx.prepare_cached("DELETE FROM track_waveforms WHERE track_id = ?1")?.execute([id])?;
    track_changed(tx, lib, id, now, touched)
}

/// Records in the feed at `now` that track `id` of `lib` changed, and adds what it points at to
/// `touched`, so the albums, artists and tags it counts toward follow.
fn track_changed(
    tx: &Transaction<'_>,
    lib: i64,
    id: i64,
    now: i64,
    touched: &mut Touched,
) -> Result<()> {
    touched.track(tx, lib, id)?;
    feed::record(tx, lib, Entity::Track, id, Op::Upsert, now)
}

/// Whether a track not yet missing, or a problem, lies under `folder`, at any depth.
pub fn indexes_under(store: &SqliteStore, lib: i64, folder: &Path) -> Result<bool, DbError> {
    let Some((root_id, rel)) = store.roots.locate(lib, folder) else {
        return Ok(false);
    };
    store.db.read_blocking(move |conn| {
        let tracks = "SELECT 1 FROM tracks WHERE root_id = ?1 AND missing_since IS NULL";
        let problems = "SELECT 1 FROM scan_problems WHERE root_id = ?1";
        match bounds_under(&rel) {
            None => conn
                .prepare_cached(&format!("SELECT EXISTS ({tracks}) OR EXISTS ({problems})"))?
                .query_row(params![root_id], |r| r.get(0)),
            Some([lower, upper]) => conn
                .prepare_cached(&format!(
                    "SELECT EXISTS ({tracks} AND path >= ?2 AND path < ?3) \
                     OR EXISTS ({problems} AND path >= ?2 AND path < ?3)"
                ))?
                .query_row(params![root_id, lower, upper], |r| r.get(0)),
        }
    })
}
