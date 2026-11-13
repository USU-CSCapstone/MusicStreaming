//! `listAccountChanges` (`api/openapi.yaml`): the caller's own change feed with each changed
//! playlist's, playlist item's, hidden track's and recent search's device rows
//! (`design/database.md` §5).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_catalog::sync::{Kind, Row, export_for};
use rusqlite::{Connection, params};
use serde::Serialize;

use super::changes::{Changes, ChangesQuery, head, next};
use super::extract::Query;
use super::scope::Caller;
use super::{Id, Problem};
use crate::db::feed::{Feed, Op};
use crate::db::{Database, reaches};

/// The spec's `AccountChange`.
#[derive(Serialize)]
pub struct Change {
    position: i64,
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
/// reaches are listed. A position this run cannot go on from is `410 cursor_expired`.
pub async fn list(
    State(db): State<Arc<Database>>,
    caller: Caller,
    Query(query): Query<ChangesQuery>,
) -> Result<Json<Changes<Change>>, Problem> {
    let (after, epoch, limit) = query.check()?;
    let current = db.epoch();
    db.try_read(move |conn| read(conn, caller, (after, epoch), limit, current)).await.map(Json)
}

/// The changes of `caller` after the cursor `(after, epoch)` as [`list`] answers them, in run
/// `current`.
fn read(
    conn: &Connection,
    caller: Caller,
    (after, epoch): (i64, Option<i64>),
    limit: u32,
    current: i64,
) -> Result<Changes<Change>, Problem> {
    let head = head(conn, Feed::Account(caller.user), epoch, after)?;
    // A library the caller lost is stepped over; its rows are not theirs to read now.
    let listed: Vec<(i64, i64, String, i64, i64, String)> = conn
        .prepare_cached(&format!(
            "SELECT c.position, c.library_id, c.kind, c.entity_id, c.item_key, c.op \
             FROM account_changes AS c WHERE c.user_id = ?1 AND c.position > ?2 AND {} \
             ORDER BY c.position LIMIT ?4",
            reaches!(admin: "?3", user: "?1", library: "c.library_id")
        ))?
        .query_map(params![caller.user, after, caller.admin, limit], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let next = next(listed.last().map(|row| row.0), listed.len(), limit, head);
    let mut changes = Vec::with_capacity(listed.len());
    for (position, library, kind, entity, item_key, op) in listed {
        let (kind, key) = match kind.as_str() {
            "playlist" => (Kind::Playlist, None),
            "hiddenTrack" => (Kind::HiddenTrack, None),
            "recentSearch" => (Kind::RecentSearch, None),
            _ => (Kind::PlaylistItem, Some(item_key)),
        };
        let (op, rows) = if op == "delete" {
            (Op::Delete, None)
        } else {
            // What an upsert names is there: its change and it were written together.
            let id = entity.to_string();
            let Some(rows) = export_for(conn, caller.user, library, kind, &id, item_key)? else {
                continue;
            };
            (Op::Upsert, Some(rows.tables))
        };
        changes.push(Change { position, op, kind, id: Id(entity), key, rows });
    }
    Ok(Changes { changes, next, head, epoch: Id(current) })
}

#[cfg(test)]
mod tests;
