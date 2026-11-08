//! `listAccountChanges` (`api/openapi.yaml`): the caller's own change feed with each changed
//! playlist's and playlist item's device rows (`design/database.md` §5).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_catalog::sync::{Kind, Row, export};
use rusqlite::{Connection, params};
use serde::Serialize;

use super::changes::{Changes, ChangesQuery, next};
use super::extract::Query;
use super::scope::Caller;
use super::{Code, Id, Problem};
use crate::db::feed::Op;
use crate::db::{Database, reaches};

/// The spec's `AccountChange`.
#[derive(Serialize)]
pub struct Change {
    seq: i64,
    op: Op,
    kind: Kind,
    id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<BTreeMap<String, Vec<Row>>>,
}

/// `listAccountChanges`: the caller's changes after `after`, at most `limit`, each upsert with
/// the playlist's or item's current rows, read from one snapshot. Only libraries the caller
/// reaches are listed. `after` beyond the caller's latest change is `410 cursor_expired`.
pub async fn list(
    State(db): State<Arc<Database>>,
    caller: Caller,
    Query(query): Query<ChangesQuery>,
) -> Result<Json<Changes<Change>>, Problem> {
    let (after, limit) = query.check()?;
    db.try_read(move |conn| read(conn, caller, after, limit)).await.map(Json)
}

/// The changes of `caller` after `after` as [`list`] answers them.
fn read(
    conn: &Connection,
    caller: Caller,
    after: i64,
    limit: u32,
) -> Result<Changes<Change>, Problem> {
    let head: i64 = conn
        .prepare_cached("SELECT coalesce(max(seq), 0) FROM account_changes WHERE user_id = ?1")?
        .query_row([caller.user], |row| row.get(0))?;
    if after > head {
        return Err(Problem::new(Code::CursorExpired));
    }
    // A library the caller lost is stepped over; its playlists are not theirs to read now.
    let listed: Vec<(i64, i64, String, i64, i64, String)> = conn
        .prepare_cached(&format!(
            "SELECT c.seq, c.library_id, c.kind, c.playlist_id, c.item_key, c.op \
             FROM account_changes AS c WHERE c.user_id = ?1 AND c.seq > ?2 AND {} \
             ORDER BY c.seq LIMIT ?4",
            reaches!(admin: "?3", user: "?1", library: "c.library_id")
        ))?
        .query_map(params![caller.user, after, caller.admin, limit], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let next = next(listed.last().map(|row| row.0), listed.len(), limit, head);
    let mut changes = Vec::with_capacity(listed.len());
    for (seq, library, kind, playlist, item_key, op) in listed {
        let (kind, key) = match kind.as_str() {
            "playlist" => (Kind::Playlist, None),
            _ => (Kind::PlaylistItem, Some(item_key)),
        };
        let (op, rows) = if op == "delete" {
            (Op::Delete, None)
        } else {
            // An upserted playlist or item is there: its change and it were written together.
            let id = playlist.to_string();
            let Some(rows) = export(conn, library, kind, &id, item_key)? else { continue };
            (Op::Upsert, Some(rows.tables))
        };
        changes.push(Change { seq, op, kind, id: Id(playlist), key, rows });
    }
    Ok(Changes { changes, next, head })
}

#[cfg(test)]
mod tests;
