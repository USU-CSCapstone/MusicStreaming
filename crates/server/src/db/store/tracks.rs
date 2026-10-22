//! Tracks as the scanner sees them: applying a batch, and whether anything is indexed under a
//! folder.

use std::path::Path;

use jewelcase_scanner::Batch;
use rusqlite::{Result, Transaction, params};

use super::SqliteStore;
use super::problems::{clear_problem, record_problem};
use super::roots::{Roots, bounds_under};
use super::upsert::upsert_track;
use crate::db::catalog::{self, Touched};
use crate::db::feed::{self, Entity, Op};
use crate::db::{DbError, now_ms};

/// Writes `batch` in one transaction: its tracks, returned files, tracks and folders gone,
/// files changed but unread, problems, and the derived data they touch. Returns how many tracks
/// it marked missing that were not already.
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
            let changed = tx.execute(
                "UPDATE tracks SET missing_since = ?3 \
                 WHERE id = ?1 AND library_id = ?2 AND missing_since IS NULL",
                params![id, lib, now],
            )?;
            if changed > 0 {
                track_changed(tx, lib, id, now, &mut touched)?;
                marked += 1;
            }
        }
        for folder in &batch.vanished {
            marked += mark_vanished(&roots, tx, lib, folder, now, &mut touched)?;
        }
        for &id in &batch.returned {
            let changed = tx.execute(
                "UPDATE tracks SET missing_since = NULL \
                 WHERE id = ?1 AND library_id = ?2 AND missing_since IS NOT NULL",
                params![id, lib],
            )?;
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
        Ok(marked)
    })
}

/// Marks every track of `lib` under `folder` that is not missing yet as missing at `now`,
/// records each in the feed and adds it to `touched`, and returns how many. A folder no root of
/// `lib` holds has nothing under it, and a root is never a vanished folder.
fn mark_vanished(
    roots: &Roots,
    tx: &Transaction<'_>,
    lib: i64,
    folder: &Path,
    now: i64,
    touched: &mut Touched,
) -> Result<i64> {
    let Some((root_id, rel)) = roots.locate(lib, folder) else {
        return Ok(0);
    };
    let Some([lower, upper]) = bounds_under(&rel) else {
        return Ok(0);
    };
    let mut stmt = tx.prepare_cached(
        "UPDATE tracks SET missing_since = ?4 \
         WHERE root_id = ?1 AND path >= ?2 AND path < ?3 AND missing_since IS NULL RETURNING id",
    )?;
    let ids: Vec<i64> = stmt
        .query_map(params![root_id, lower, upper, now], |r| r.get(0))?
        .collect::<Result<_>>()?;
    let mut marked = 0;
    for id in ids {
        track_changed(tx, lib, id, now, touched)?;
        marked += 1;
    }
    Ok(marked)
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
    let changed = tx.execute(
        "UPDATE tracks SET file_mtime = NULL, analyzer_version = 0, loudness_lufs = NULL, \
         peak_dbtp = NULL WHERE id = ?1 AND library_id = ?2 AND file_mtime IS NOT NULL",
        params![id, lib],
    )?;
    if changed == 0 {
        return Ok(());
    }
    tx.execute("DELETE FROM track_waveforms WHERE track_id = ?1", [id])?;
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
            None => conn.query_row(
                &format!("SELECT EXISTS ({tracks}) OR EXISTS ({problems})"),
                params![root_id],
                |r| r.get(0),
            ),
            Some([lower, upper]) => conn.query_row(
                &format!(
                    "SELECT EXISTS ({tracks} AND path >= ?2 AND path < ?3) \
                     OR EXISTS ({problems} AND path >= ?2 AND path < ?3)"
                ),
                params![root_id, lower, upper],
                |r| r.get(0),
            ),
        }
    })
}
