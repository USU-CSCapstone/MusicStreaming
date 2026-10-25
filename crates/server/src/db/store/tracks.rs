//! Tracks as the scanner sees them: applying a batch.

use jewelcase_scanner::Batch;
use rusqlite::{Result, Transaction, params};

use super::SqliteStore;
use super::problems::{clear_problem, record_problem};
use super::scans;
use super::upsert::upsert_track;
use crate::db::catalog::{self, Touched};
use crate::db::feed::{self, Entity, Op};
use crate::db::{DbError, now_ms};

/// Writes `batch` in one transaction: its tracks, returned files, tracks gone, problems, the
/// derived data they touch, and the scan row it carries, with the row's missing count raised by
/// the tracks the batch marked missing that were not already. Returns that number.
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
