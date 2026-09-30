//! Scans: what each one covers, how far it got, and where it resumes (`design/database.md` §8).

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use jewelcase_scanner::*;
use rusqlite::{Connection, Result, params};

use super::SqliteStore;
use crate::db::{DbError, now_ms};

pub fn create(store: &SqliteStore, lib: i64, scan: &Scan) -> Result<ScanId, DbError> {
    let scan = scan.clone();
    let scopes = serde_json::to_string(&scan.scopes).unwrap_or_else(|_| "[]".into());
    store.db.write_blocking(move |tx| {
        tx.query_row(
            "INSERT INTO scans (id, library_id, trigger, state, scopes, cursor_scope, cursor_dir, \
             files_seen, files_processed, added, updated, moved, missing, problems, current_path, \
             created_at, started_at, finished_at) \
             VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17) \
             RETURNING id",
            params![
                lib,
                trigger_str(scan.trigger),
                state_str(scan.state),
                scopes,
                scan.cursor.as_ref().map(|c| c.scope_index as i64),
                scan.cursor
                    .as_ref()
                    .and_then(|c| c.after_directory.as_ref())
                    .map(|p| p.to_string_lossy().into_owned()),
                scan.progress.files_seen as i64,
                scan.progress.files_processed as i64,
                scan.progress.added as i64,
                scan.progress.updated as i64,
                scan.progress.moved as i64,
                scan.progress.missing as i64,
                scan.progress.problems as i64,
                scan.progress
                    .current_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
                now_ms(),
                time_to_ms(scan.started_at),
                time_to_ms(scan.finished_at),
            ],
            |r| r.get::<_, i64>(0).map(|id| id as u64),
        )
    })
}

pub fn update(store: &SqliteStore, scan: &Scan) -> Result<(), DbError> {
    let scan = scan.clone();
    store.db.write_blocking(move |tx| {
        tx.execute(
        "UPDATE scans SET state = ?2, cursor_scope = ?3, cursor_dir = ?4, files_seen = ?5, files_processed = ?6, \
         added = ?7, updated = ?8, moved = ?9, missing = ?10, problems = ?11, current_path = ?12, \
         started_at = ?13, finished_at = ?14 WHERE id = ?1",
        params![
            scan.id as i64,
            state_str(scan.state),
            scan.cursor.as_ref().map(|c| c.scope_index as i64),
            scan.cursor
                .as_ref()
                .and_then(|c| c.after_directory.as_ref())
                .map(|p| p.to_string_lossy().into_owned()),
            scan.progress.files_seen as i64,
            scan.progress.files_processed as i64,
            scan.progress.added as i64,
            scan.progress.updated as i64,
            scan.progress.moved as i64,
            scan.progress.missing as i64,
            scan.progress.problems as i64,
            scan.progress
                .current_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            time_to_ms(scan.started_at),
            time_to_ms(scan.finished_at),
        ],
    )
    })
    .map(|_| ())
}

/// The library's scans that were queued or running when the server last stopped.
pub fn interrupted(store: &SqliteStore, lib: i64) -> Result<Vec<Scan>, DbError> {
    store.db.read_blocking(move |conn| {
        read_scans(
            conn,
            "SELECT id, library_id, trigger, state, scopes, cursor_scope, cursor_dir, files_seen, files_processed, \
             added, updated, moved, missing, problems, current_path, started_at, finished_at \
             FROM scans WHERE library_id = ?1 AND state IN ('queued', 'running') ORDER BY created_at, id",
            lib,
        )
    })
}

impl SqliteStore {
    /// All scans for a library, newest first. For the admin API and tests.
    pub fn scans(&self, library: i64) -> Result<Vec<Scan>, DbError> {
        self.db.read_blocking(move |conn| {
            read_scans(
                conn,
                "SELECT id, library_id, trigger, state, scopes, cursor_scope, cursor_dir, files_seen, files_processed, \
                 added, updated, moved, missing, problems, current_path, started_at, finished_at \
                 FROM scans WHERE library_id = ?1 ORDER BY created_at DESC, id DESC",
                library,
            )
        })
    }
}

fn read_scans(conn: &Connection, sql: &str, lib: i64) -> Result<Vec<Scan>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([lib], |r| {
        let scopes: String = r.get(4)?;
        let cursor_scope: Option<i64> = r.get(5)?;
        let cursor_dir: Option<String> = r.get(6)?;
        Ok(Scan {
            id: r.get::<_, i64>(0)? as u64,
            library: r.get::<_, i64>(1)?.to_string(),
            trigger: trigger_parse(&r.get::<_, String>(2)?),
            state: state_parse(&r.get::<_, String>(3)?),
            scopes: serde_json::from_str(&scopes).unwrap_or_default(),
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

fn ms_to_time(ms: Option<i64>) -> Option<SystemTime> {
    ms.map(|m| SystemTime::UNIX_EPOCH + Duration::from_millis(m.max(0) as u64))
}

fn time_to_ms(t: Option<SystemTime>) -> Option<i64> {
    t.map(|t| system_time_ms(t) as i64)
}

fn trigger_str(t: Trigger) -> &'static str {
    match t {
        Trigger::Initial => "initial",
        Trigger::Watch => "watch",
        Trigger::Scheduled => "scheduled",
        Trigger::Manual => "manual",
        Trigger::Reconfigure => "reconfigure",
        Trigger::Restore => "restore",
    }
}

fn trigger_parse(s: &str) -> Trigger {
    match s {
        "initial" => Trigger::Initial,
        "watch" => Trigger::Watch,
        "scheduled" => Trigger::Scheduled,
        "reconfigure" => Trigger::Reconfigure,
        "restore" => Trigger::Restore,
        _ => Trigger::Manual,
    }
}

fn state_str(s: ScanState) -> &'static str {
    match s {
        ScanState::Queued => "queued",
        ScanState::Running => "running",
        ScanState::Completed => "completed",
        ScanState::Cancelled => "cancelled",
        ScanState::Suspended => "suspended",
    }
}

fn state_parse(s: &str) -> ScanState {
    match s {
        "queued" => ScanState::Queued,
        "running" => ScanState::Running,
        "cancelled" => ScanState::Cancelled,
        "suspended" => ScanState::Suspended,
        _ => ScanState::Completed,
    }
}
