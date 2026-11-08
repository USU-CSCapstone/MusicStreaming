//! `listLibraryChanges` (`api/openapi.yaml`): the library change feed with each changed
//! entity's device rows (`design/general.md` §3, §4).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_catalog::sync::{Kind, Row, export};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use super::extract::Query;
use super::page;
use super::scope::Scope;
use super::{Code, Id, Problem};
use crate::db::Database;
use crate::db::feed::Op;

/// The parameters of `listLibraryChanges`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangesQuery {
    #[serde(default)]
    after: i64,
    limit: Option<u32>,
}

/// The spec's `LibraryChanges`.
#[derive(Serialize)]
pub struct Changes {
    changes: Vec<Change>,
    next: i64,
    head: i64,
}

/// The spec's `LibraryChange`.
#[derive(Serialize)]
pub struct Change {
    seq: i64,
    op: Op,
    kind: Kind,
    id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<BTreeMap<String, Vec<Row>>>,
}

/// `listLibraryChanges`: the library's changes after `after`, at most `limit`, each upsert with
/// its entity's current rows, read from one snapshot. `after` beyond the library's latest change
/// is `410 cursor_expired`.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Query(query): Query<ChangesQuery>,
) -> Result<Json<Changes>, Problem> {
    let limit = page::limit(query.limit)?;
    if query.after < 0 {
        return Err(Problem::invalid("after must be at least 0"));
    }
    db.try_read(move |conn| read(conn, library, query.after, limit)).await.map(Json)
}

/// The changes of `library` after `after` as [`list`] answers them.
fn read(conn: &Connection, library: i64, after: i64, limit: u32) -> Result<Changes, Problem> {
    let head: i64 = conn
        .prepare_cached("SELECT coalesce(max(seq), 0) FROM library_changes WHERE library_id = ?1")?
        .query_row([library], |row| row.get(0))?;
    if after > head {
        return Err(Problem::new(Code::CursorExpired));
    }
    // The device keeps no library row, so those changes are stepped over.
    let listed: Vec<(i64, String, i64, String)> = conn
        .prepare_cached(
            "SELECT seq, entity_type, entity_id, op FROM library_changes \
             WHERE library_id = ?1 AND seq > ?2 AND entity_type <> 'library' \
             ORDER BY seq LIMIT ?3",
        )?
        .query_map(params![library, after, limit], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let next = match listed.last() {
        Some(&(seq, ..)) if listed.len() == limit as usize => seq,
        _ => head,
    };
    let mut changes = Vec::with_capacity(listed.len());
    for (seq, entity, id, op) in listed {
        let kind = match entity.as_str() {
            "track" => Kind::Track,
            "album" => Kind::Album,
            "artist" => Kind::Artist,
            _ => Kind::Tag,
        };
        let (op, rows) = if op == "delete" {
            (Op::Delete, None)
        } else {
            // In the snapshot an upserted entity is always there: its change and it were
            // written together.
            let Some(rows) = export(conn, library, kind, &id.to_string(), 0)? else { continue };
            (Op::Upsert, Some(rows.tables))
        };
        changes.push(Change { seq, op, kind, id: Id(id), rows });
    }
    Ok(Changes { changes, next, head })
}

#[cfg(test)]
mod tests;
