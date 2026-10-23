//! Scans: what each one covers, how far it got, and where it resumes (`design/database.md` §8).
//! A row keeps each path as a root's id and the path relative to it, as tracks do.

mod stored;

use jewelcase_scanner::{Cursor, Scan, ScanProgress, ScanState, Scope, Trigger};
use rusqlite::Error::FromSqlConversionFailure;
use rusqlite::types::Type;
use rusqlite::{Connection, Result, Row, Transaction, params};

use self::stored::{cursor_dir, scope_at, scopes, stored_scopes};
use super::SqliteStore;
use super::roots::{Roots, join};
use crate::db::{DbError, Text, new_id, now_ms};

/// Records a new scan of `scopes` in `lib` under `trigger`, queued, and returns it.
pub fn create(
    store: &SqliteStore,
    lib: i64,
    trigger: Trigger,
    scopes: &[Scope],
) -> Result<Scan, DbError> {
    let json = stored_scopes(&store.roots, lib, scopes)?;
    let scopes = scopes.to_vec();
    store.db.write_blocking(move |tx| {
        let id = new_id()?;
        tx.execute(
            "INSERT INTO scans (id, library_id, trigger, state, scopes, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, lib, Text(trigger), Text(ScanState::Queued), json, now_ms()],
        )?;
        Ok(Scan::queued(id, lib, trigger, scopes))
    })
}

/// Saves what changes as `scan` runs.
pub fn update(store: &SqliteStore, scan: &Scan) -> Result<(), DbError> {
    let (roots, scan) = (store.roots.clone(), scan.clone());
    store.db.write_blocking(move |tx| save(tx, &roots, &scan))
}

/// The library's scans that were queued or running when the server last stopped. A row that
/// cannot be read, or has a scope whose root is no longer one of the library's, is marked
/// suspended, since it can never resume; if even that fails, the error is logged and the row is
/// left as it was.
pub fn interrupted(store: &SqliteStore, lib: i64) -> Result<Vec<Scan>, DbError> {
    let roots = store.roots.clone();
    let (scans, unreadable) = store.db.read_blocking(move |conn| {
        read(conn, &roots, lib, "AND state IN ('queued', 'running') ORDER BY created_at, id")
    })?;
    if !unreadable.is_empty() {
        let suspended = store.db.write_blocking(move |tx| {
            let mut suspend =
                tx.prepare("UPDATE scans SET state = 'suspended', finished_at = ?2 WHERE id = ?1")?;
            for id in unreadable {
                suspend.execute(params![id, now_ms()])?;
            }
            Ok(())
        });
        if let Err(error) = suspended {
            tracing::error!(library = lib, %error, "unreadable scan rows not marked suspended");
        }
    }
    Ok(scans)
}

#[cfg(test)]
impl SqliteStore {
    /// All scans of a library, newest first, for the tests to inspect.
    pub fn scans(&self, library: i64) -> Result<Vec<Scan>, DbError> {
        let (order, roots) = ("ORDER BY created_at DESC, id DESC", self.roots.clone());
        self.db.read_blocking(move |conn| Ok(read(conn, &roots, library, order)?.0))
    }
}

/// Writes what changes as a scan runs: its state, cursor, progress, and times, and the folder it
/// is in, if any, as one of `roots` and the path relative to it. Only a row in the scan's own
/// library is changed: a scan with no row there is an error, so its writes are not committed.
pub(super) fn save(tx: &Transaction, roots: &Roots, scan: &Scan) -> Result<()> {
    let progress = &scan.progress;
    let current = progress.current_path.as_ref();
    let (current_root, current_path) =
        current.and_then(|path| roots.locate(scan.library, path)).unzip();
    let changed = tx.prepare_cached(
        "UPDATE scans SET state = ?2, cursor_scope = ?3, cursor_dir = ?4, files_seen = ?5, \
         files_processed = ?6, added = ?7, updated = ?8, moved = ?9, missing = ?10, problems = ?11, \
         current_root_id = ?12, current_path = ?13, started_at = ?14, finished_at = ?15 \
         WHERE id = ?1 AND library_id = ?16",
    )?
    .execute(params![
        scan.id,
        Text(scan.state),
        scan.cursor.as_ref().map(|c| c.scope_index),
        cursor_dir(scan),
        progress.files_seen,
        progress.files_processed,
        progress.added,
        progress.updated,
        progress.moved,
        progress.missing,
        progress.problems,
        current_root,
        current_path,
        scan.started_at,
        scan.finished_at,
        scan.library,
    ])?;
    if changed == 1 { Ok(()) } else { Err(rusqlite::Error::StatementChangedRows(changed)) }
}

/// The library's scans that `rest`, such as an `ORDER BY`, selects, with their paths under
/// `roots`, and the ids of those whose rows cannot be read. Each of those is logged and left out,
/// so one bad row hides only itself.
fn read(conn: &Connection, roots: &Roots, lib: i64, rest: &str) -> Result<(Vec<Scan>, Vec<i64>)> {
    let sql = format!(
        "SELECT id, library_id, trigger, state, scopes, cursor_scope, cursor_dir, files_seen, \
         files_processed, added, updated, moved, missing, problems, current_root_id, \
         current_path, started_at, finished_at FROM scans WHERE library_id = ?1 {rest}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([lib], |r| match scan(r, roots, lib) {
        Ok(found) => Ok(Ok(found)),
        Err(error) => {
            let id: i64 = r.get(0)?;
            tracing::error!(library = lib, scan = id, %error, "scan row cannot be read; skipping it");
            Ok(Err(id))
        }
    })?;
    let (mut scans, mut unreadable) = (Vec::new(), Vec::new());
    for row in rows {
        match row? {
            Ok(found) => scans.push(found),
            Err(id) => unreadable.push(id),
        }
    }
    Ok((scans, unreadable))
}

/// The scan of `lib` in row `r`, in the columns [`read`] selects, with its paths under `roots`.
fn scan(r: &Row, roots: &Roots, lib: i64) -> Result<Scan> {
    let scopes = scopes(r, 4, roots, lib)?;
    let cursor = match r.get::<_, Option<i64>>(5)? {
        None => None,
        Some(scope_index) => {
            let after_directory = r.get::<_, Option<String>>(6)?.map(|rel| -> Result<_> {
                let scope = scope_at(&scopes, scope_index).ok_or_else(|| {
                    FromSqlConversionFailure(6, Type::Text, "the cursor is in no scope".into())
                })?;
                Ok(join(&scope.root, &rel))
            });
            Some(Cursor { scope_index, after_directory: after_directory.transpose()? })
        }
    };
    let current: Option<(i64, String)> = Option::zip(r.get(14)?, r.get(15)?);
    Ok(Scan {
        id: r.get(0)?,
        library: r.get(1)?,
        trigger: r.get::<_, Text<_>>(2)?.0,
        state: r.get::<_, Text<_>>(3)?.0,
        scopes,
        cursor,
        progress: ScanProgress {
            files_seen: r.get(7)?,
            files_processed: r.get(8)?,
            added: r.get(9)?,
            updated: r.get(10)?,
            moved: r.get(11)?,
            missing: r.get(12)?,
            problems: r.get(13)?,
            current_path: current.and_then(|(root, rel)| roots.absolute(lib, root, &rel)),
        },
        started_at: r.get(16)?,
        finished_at: r.get(17)?,
    })
}
