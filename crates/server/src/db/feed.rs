//! The library change feed (`design/database.md` §5): one row per entity,
//! coalesced on write, in the same transaction as the change.

use rusqlite::{Connection, Result, params};
use serde::Serialize;

use super::{Text, plugin_view};

/// What changed, stored by its name in `library_changes.entity_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Entity {
    Library,
    Track,
    Album,
    Artist,
    Tag,
}

/// How it changed, stored by its name in `library_changes.op`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Upsert,
    Delete,
}

/// Records that `entity` changed. Its earlier row, if any, is replaced, so the feed carries only
/// the latest position for each entity. An upsert that leaves what plugins see of the entity as
/// it was ([`plugin_view::fingerprint`]) keeps the position plugins last saw it change at, so it
/// moves the entity for devices without waking the change hooks.
pub fn record(
    conn: &Connection,
    library_id: i64,
    entity: Entity,
    id: i64,
    op: Op,
    now: i64,
) -> Result<()> {
    let view = match op {
        Op::Upsert => plugin_view::fingerprint(conn, library_id, entity, id)?,
        Op::Delete => None,
    };
    // A NULL view never equals one, so a deletion, a kind no hook reads, and a row from before
    // views were kept all count as seen by plugins. The old row is read before REPLACE drops it.
    conn.prepare_cached(
        "INSERT OR REPLACE INTO library_changes (library_id, entity_type, entity_id, op, \
         changed_at, plugin_view, plugin_since) VALUES (?1, ?2, ?3, ?4, ?5, ?6, \
         (SELECT plugin_seq FROM library_changes \
          WHERE library_id = ?1 AND entity_type = ?2 AND entity_id = ?3 AND plugin_view = ?6))",
    )?
    .execute(params![library_id, Text(entity), id, Text(op), now, view])?;
    Ok(())
}
