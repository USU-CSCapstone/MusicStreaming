//! Scans: what each one covers, how far it got, and where it resumes (`design/database.md` §8).

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use jewelcase_scanner::{Cursor, Scan, ScanId, ScanProgress, system_time_ms};
use rusqlite::Error::{FromSqlConversionFailure, ToSqlConversionFailure};
use rusqlite::types::Type;
use rusqlite::{Connection, Result, Row, Transaction, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::SqliteStore;
use crate::db::{DbError, now_ms};

pub fn create(store: &SqliteStore, lib: i64, scan: &Scan) -> Result<ScanId, DbError> {
    let scan = scan.clone();
    store.db.write_blocking(move |tx| {
        // A path that is not UTF-8 cannot be written: better no record of the scan than one
        // that resumes as a scan of nothing.
        let scopes = serde_json::to_string(&scan.scopes)
            .map_err(|error| ToSqlConversionFailure(Box::new(error)))?;
        let id = tx.query_row(
            "INSERT INTO scans (id, library_id, trigger, state, scopes, created_at) \
             VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, ?5) RETURNING id",
            params![lib, name(scan.trigger), name(scan.state), scopes, now_ms()],
            |r| r.get(0),
        )?;
        save(tx, id, &scan)?;
        Ok(id as ScanId)
    })
}

pub fn update(store: &SqliteStore, scan: &Scan) -> Result<(), DbError> {
    let scan = scan.clone();
    store
        .db
        .write_blocking(move |tx| save(tx, scan.id as i64, &scan))
}

/// The library's scans that were queued or running when the server last stopped.
pub fn interrupted(store: &SqliteStore, lib: i64) -> Result<Vec<Scan>, DbError> {
    store.db.read_blocking(move |conn| {
        read(
            conn,
            lib,
            "AND state IN ('queued', 'running') ORDER BY created_at, id",
        )
    })
}

#[cfg(test)]
impl SqliteStore {
    /// All scans for a library, newest first. Only the tests read them until the admin API.
    pub fn scans(&self, library: i64) -> Result<Vec<Scan>, DbError> {
        self.db
            .read_blocking(move |conn| read(conn, library, "ORDER BY created_at DESC, id DESC"))
    }
}

/// Writes what changes as a scan runs: its state, cursor, progress, and times.
fn save(tx: &Transaction, id: i64, scan: &Scan) -> Result<()> {
    let path = |path: Option<&PathBuf>| path.map(|p| p.to_string_lossy().into_owned());
    let progress = &scan.progress;
    tx.execute(
        "UPDATE scans SET state = ?2, cursor_scope = ?3, cursor_dir = ?4, files_seen = ?5, \
         files_processed = ?6, added = ?7, updated = ?8, moved = ?9, missing = ?10, problems = ?11, \
         current_path = ?12, started_at = ?13, finished_at = ?14 WHERE id = ?1",
        params![
            id,
            name(scan.state),
            scan.cursor.as_ref().map(|c| c.scope_index as i64),
            path(scan.cursor.as_ref().and_then(|c| c.after_directory.as_ref())),
            progress.files_seen as i64,
            progress.files_processed as i64,
            progress.added as i64,
            progress.updated as i64,
            progress.moved as i64,
            progress.missing as i64,
            progress.problems as i64,
            path(progress.current_path.as_ref()),
            time_to_ms(scan.started_at),
            time_to_ms(scan.finished_at),
        ],
    )?;
    Ok(())
}

/// The library's scans that `rest`, such as an `ORDER BY`, selects.
fn read(conn: &Connection, lib: i64, rest: &str) -> Result<Vec<Scan>> {
    let sql = format!(
        "SELECT id, library_id, trigger, state, scopes, cursor_scope, cursor_dir, files_seen, \
         files_processed, added, updated, moved, missing, problems, current_path, started_at, \
         finished_at FROM scans WHERE library_id = ?1 {rest}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([lib], |r| {
        let scopes: String = r.get(4)?;
        let scopes = serde_json::from_str(&scopes)
            .map_err(|error| FromSqlConversionFailure(4, Type::Text, Box::new(error)))?;
        let cursor_scope: Option<i64> = r.get(5)?;
        let cursor_dir: Option<String> = r.get(6)?;
        Ok(Scan {
            id: r.get::<_, i64>(0)? as u64,
            library: r.get::<_, i64>(1)?.to_string(),
            trigger: parse(r, 2)?,
            state: parse(r, 3)?,
            scopes,
            cursor: cursor_scope.map(|s| Cursor {
                scope_index: s as usize,
                after_directory: cursor_dir.map(PathBuf::from),
            }),
            progress: ScanProgress {
                files_seen: r.get::<_, i64>(7)? as u64,
                files_processed: r.get::<_, i64>(8)? as u64,
                added: r.get::<_, i64>(9)? as u64,
                updated: r.get::<_, i64>(10)? as u64,
                moved: r.get::<_, i64>(11)? as u64,
                missing: r.get::<_, i64>(12)? as u64,
                problems: r.get::<_, i64>(13)? as u64,
                current_path: r.get::<_, Option<String>>(14)?.map(PathBuf::from),
            },
            started_at: ms_to_time(r.get(15)?),
            finished_at: ms_to_time(r.get(16)?),
        })
    })?;
    rows.collect()
}

/// A trigger's or state's name as the API spells it, which is also what the schema's `CHECK`
/// allows.
fn name(value: impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => unreachable!("a unit variant serializes as its name"),
    }
}

/// The trigger or state named in column `index`.
fn parse<T: DeserializeOwned>(row: &Row, index: usize) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(row.get(index)?))
        .map_err(|error| FromSqlConversionFailure(index, Type::Text, Box::new(error)))
}

fn ms_to_time(ms: Option<i64>) -> Option<SystemTime> {
    ms.map(|m| SystemTime::UNIX_EPOCH + Duration::from_millis(m.max(0) as u64))
}

fn time_to_ms(t: Option<SystemTime>) -> Option<i64> {
    t.map(|t| system_time_ms(t) as i64)
}

#[cfg(test)]
mod tests {
    use jewelcase_scanner::{ScanState, Trigger};

    use super::name;

    /// The names `0001_libraries.sql` allows in `scans.trigger` and `scans.state`.
    #[test]
    fn every_name_is_one_the_schema_allows() {
        use ScanState::*;
        use Trigger::*;
        assert_eq!(
            [Initial, Watch, Scheduled, Manual, Reconfigure, Restore].map(name),
            [
                "initial",
                "watch",
                "scheduled",
                "manual",
                "reconfigure",
                "restore"
            ]
        );
        assert_eq!(
            [Queued, Running, Completed, Cancelled, Suspended].map(name),
            ["queued", "running", "completed", "cancelled", "suspended"]
        );
    }
}
