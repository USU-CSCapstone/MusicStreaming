//! Tracks as the scanner sees them: looking a file up, applying a batch, marking what a scan did
//! not see as missing, and whether anything is indexed under a folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use jewelcase_scanner::store::{Batch, IndexedFile, IndexedLyrics};
use jewelcase_scanner::{Depth, ScanId, Scope};
use rusqlite::{Connection, OptionalExtension, Result, ToSql, Transaction, params};

use super::SqliteStore;
use super::problems::{clear_problem, clear_unopened, record_problem};
use super::roots::{bounds_under, relative};
use super::upsert::upsert_track;
use crate::db::catalog::{self, Touched};
use crate::db::feed::{self, Entity, Op};
use crate::db::{DbError, now_ms};

/// What the index holds for the file at `path` in `lib`, or `None` when no root of `lib` holds
/// it or no track is there.
pub fn lookup(store: &SqliteStore, lib: i64, path: &Path) -> Result<Option<IndexedFile>, DbError> {
    let Some((root_id, rel)) = store.roots.locate(lib, path) else {
        return Ok(None);
    };
    store.db.read_blocking(move |conn| {
        conn.query_row(
            "SELECT t.id, t.file_size, t.file_mtime, t.missing_since, l.embedded \
             FROM tracks t LEFT JOIN track_lyrics l ON l.track_id = t.id \
             WHERE t.root_id = ?1 AND t.path = ?2",
            params![root_id, rel],
            |r| {
                Ok(IndexedFile {
                    track_id: r.get(0)?,
                    size: r.get::<_, i64>(1)? as u64,
                    mtime_ms: r.get::<_, Option<i64>>(2)?.map(|m| m as u64),
                    missing: r.get::<_, Option<i64>>(3)?.is_some(),
                    // So a lyrics file added or removed beside an unchanged track is noticed.
                    lyrics: match r.get::<_, Option<i64>>(4)? {
                        None => IndexedLyrics::None,
                        Some(1) => IndexedLyrics::Embedded,
                        Some(_) => IndexedLyrics::Sidecar,
                    },
                })
            },
        )
        .optional()
    })
}

/// Writes `batch` in one transaction: its tracks, returned files, files changed but unread,
/// problems, and the derived data they touch.
pub fn apply(store: &SqliteStore, lib: i64, batch: Batch) -> Result<(), DbError> {
    let roots = store.roots.clone();
    store.db.write_blocking(move |tx| {
        let now = now_ms();
        let mut touched = Touched::default();
        for record in &batch.upserts {
            upsert_track(&roots, tx, lib, record, batch.scan_id, now, &mut touched)?;
        }
        for id in batch.touched.iter().chain(&batch.returned) {
            tx.execute(
                "UPDATE tracks SET last_seen_scan_id = ?2 WHERE id = ?1 AND library_id = ?3",
                params![id, batch.scan_id, lib],
            )?;
        }
        for &id in &batch.returned {
            let changed = tx.execute(
                "UPDATE tracks SET missing_since = NULL \
                 WHERE id = ?1 AND library_id = ?2 AND missing_since IS NOT NULL",
                params![id, lib],
            )?;
            if changed > 0 {
                touched.track(tx, lib, id)?;
                feed::record(tx, lib, Entity::Track, id, Op::Upsert, now)?;
            }
        }
        for &id in &batch.changed_unread {
            forget_stamp(tx, lib, id, now, &mut touched)?;
        }
        for path in &batch.cleared {
            clear_problem(&roots, tx, lib, path)?;
        }
        for path in &batch.opened {
            clear_unopened(&roots, tx, lib, path)?;
        }
        for problem in &batch.problems {
            record_problem(&roots, tx, lib, problem)?;
        }
        catalog::recompute(tx, lib, &touched, now)
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
    let changed = tx.execute(
        "UPDATE tracks SET file_mtime = NULL, analyzer_version = 0, loudness_lufs = NULL, \
         peak_dbtp = NULL WHERE id = ?1 AND library_id = ?2 AND file_mtime IS NOT NULL",
        params![id, lib],
    )?;
    if changed == 0 {
        return Ok(());
    }
    tx.execute("DELETE FROM track_waveforms WHERE track_id = ?1", [id])?;
    touched.track(tx, lib, id)?;
    feed::record(tx, lib, Entity::Track, id, Op::Upsert, now)
}

/// Marks the tracks in `scope` that scan `scan_id` did not see as missing, other than those at
/// or under a path in `unreadable`, and returns how many.
pub fn mark_missing_unseen(
    store: &SqliteStore,
    lib: i64,
    scope: &Scope,
    scan_id: ScanId,
    unreadable: &[PathBuf],
) -> Result<i64, DbError> {
    let Some(root_id) = store.roots.root_by_path(lib, &scope.root) else {
        return Ok(0);
    };
    let rel = relative(&scope.root, &scope.path);
    let unreadable: HashSet<PathBuf> =
        unreadable.iter().map(|u| relative(&scope.root, u).into()).collect();
    let depth = scope.depth;
    store.db.write_blocking(move |tx| {
        let now = now_ms();
        let unseen = "SELECT id, path FROM tracks WHERE root_id = ?1 AND missing_since IS NULL \
                      AND (last_seen_scan_id IS NULL OR last_seen_scan_id <> ?2)";
        let unseen_rows = match bounds_under(&rel) {
            None => {
                let sql = match depth {
                    Depth::Subtree => unseen.to_owned(),
                    Depth::Directory => format!("{unseen} AND instr(path, '/') = 0"),
                };
                unseen_tracks(tx, &sql, params![root_id, scan_id])?
            }
            Some([lower, upper]) => {
                // The Directory term measures the prefix with SQL `length`, which counts
                // characters as `substr` does; a byte length would skip past the start of the
                // remainder on non-ASCII folder names.
                let sql = match depth {
                    Depth::Subtree => format!("{unseen} AND path >= ?3 AND path < ?4"),
                    Depth::Directory => format!(
                        "{unseen} AND path >= ?3 AND path < ?4 \
                         AND instr(substr(path, length(?3) + 1), '/') = 0"
                    ),
                };
                unseen_tracks(tx, &sql, params![root_id, scan_id, lower, upper])?
            }
        };
        // The unseen rows are already in hand, so the paths the scan could not read are matched
        // here rather than in SQL.
        let readable = unseen_rows
            .into_iter()
            .filter(|(_, path)| !Path::new(path).ancestors().any(|a| unreadable.contains(a)));
        let mut touched = Touched::default();
        let mut marked = 0;
        for (id, _) in readable {
            tx.execute("UPDATE tracks SET missing_since = ?2 WHERE id = ?1", params![id, now])?;
            touched.track(tx, lib, id)?;
            feed::record(tx, lib, Entity::Track, id, Op::Upsert, now)?;
            marked += 1;
        }
        catalog::recompute(tx, lib, &touched, now)?;
        Ok(marked)
    })
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

/// Runs `sql` with `args` bound and returns its rows as ids and root-relative paths.
fn unseen_tracks(conn: &Connection, sql: &str, args: &[&dyn ToSql]) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(sql)?;
    stmt.query_map(args, |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}
