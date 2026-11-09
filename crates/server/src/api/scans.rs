//! A library's scans, for admins: starting one, and seeing what is under way and how the last
//! one ended (`requirements/scanning.md` §4–5). Admins and the owner only, as everything under
//! `/admin` is (`authenticate`).

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use jewelcase_scanner::{ScanId, ScanState, Scanner, Trigger};
use rusqlite::{OptionalExtension, Row};
use serde::Serialize;

use super::scope::Scope;
use super::time::rfc3339;
use super::{Code, Id, Problem};
use crate::db::{Database, Text};

/// The spec's `Scan`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scan {
    id: Id,
    trigger: Trigger,
    state: ScanState,
    started_at: Option<String>,
    finished_at: Option<String>,
    files_seen: i64,
    files_processed: i64,
    added: i64,
    updated: i64,
    missing: i64,
    problems: i64,
    /// The folder being scanned, relative to its root: never a path on the server
    /// (`requirements/general.md` §3.7).
    current_folder: Option<String>,
}

/// `adminListScans`' response.
#[derive(Serialize)]
pub struct Scans {
    items: Vec<Scan>,
}

/// The columns [`scan`] reads.
const SELECT: &str = "SELECT id, trigger, state, started_at, finished_at, files_seen, \
     files_processed, added, updated, missing, problems, current_path FROM scans";

/// `adminStartScan`: queues a scan of every root of the library, and answers the scan that will
/// do it. A scan already queued that covers the library is that scan, so pressing again while one
/// waits queues nothing more (`requirements/scanning.md` §4).
pub async fn start(
    State(db): State<Arc<Database>>,
    State(scanners): State<Arc<HashMap<i64, Scanner>>>,
    Scope { library, .. }: Scope,
) -> Result<(StatusCode, Json<Scan>), Problem> {
    // Every library has a scanner from the start; one added since has none until a restart.
    let Some(scanner) = scanners.get(&library).cloned() else {
        return Err(Problem::new(Code::Internal).detail("This library's scanner is not running."));
    };
    // Queuing records the scan with a blocking write, which must not run on an async thread.
    let queued = tokio::task::spawn_blocking(move || scanner.scan_library(Trigger::Manual))
        .await
        .map_err(|error| {
        tracing::error!(library, %error, "queuing a manual scan failed");
        Problem::new(Code::Internal)
    })?;
    let Some(id) = queued else {
        return Err(Problem::new(Code::Internal).detail("The scan could not be recorded."));
    };
    let found = db.read(move |conn| one(conn, library, id)).await?;
    // Retention never removes a scan that has not finished, so only a row gone some other way,
    // such as by the library's deletion, is missing.
    let scan = found.ok_or_else(Problem::not_found)?;
    Ok((StatusCode::ACCEPTED, Json(scan)))
}

/// `adminListScans`: the library's queued and running scans, and the last one to finish, newest
/// first. The running scan and any queued after it are what is under way; the finished one is
/// how the library was last scanned, which retention always keeps (`db::retention`).
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
) -> Result<Json<Scans>, Problem> {
    let items = db
        .read(move |conn| {
            conn.prepare_cached(&format!(
                "{SELECT} WHERE library_id = ?1 AND (state IN ('queued', 'running') \
                 OR id = (SELECT id FROM scans WHERE library_id = ?1 AND finished_at IS NOT NULL \
                          ORDER BY created_at DESC, id DESC LIMIT 1)) \
                 ORDER BY created_at DESC, id DESC"
            ))?
            .query_map([library], scan)?
            .collect()
        })
        .await?;
    Ok(Json(Scans { items }))
}

/// Scan `id` of `library`, if there is one.
fn one(conn: &rusqlite::Connection, library: i64, id: ScanId) -> rusqlite::Result<Option<Scan>> {
    conn.prepare_cached(&format!("{SELECT} WHERE library_id = ?1 AND id = ?2"))?
        .query_row([library, id], scan)
        .optional()
}

/// A scan from a row of [`SELECT`].
fn scan(row: &Row) -> rusqlite::Result<Scan> {
    Ok(Scan {
        id: Id(row.get(0)?),
        trigger: row.get::<_, Text<_>>(1)?.0,
        state: row.get::<_, Text<_>>(2)?.0,
        started_at: row.get::<_, Option<i64>>(3)?.map(rfc3339),
        finished_at: row.get::<_, Option<i64>>(4)?.map(rfc3339),
        files_seen: row.get(5)?,
        files_processed: row.get(6)?,
        added: row.get(7)?,
        updated: row.get(8)?,
        missing: row.get(9)?,
        problems: row.get(10)?,
        current_folder: row.get(11)?,
    })
}
