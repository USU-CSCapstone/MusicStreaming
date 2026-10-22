//! The library change feed (`design/database.md` §5): one row per entity,
//! coalesced on write, in the same transaction as the change.

use rusqlite::{Connection, Result, params};
use serde::Serialize;

use super::Text;

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

/// Record that `entity` changed. Its earlier row, if any, is replaced so
/// the feed carries only the latest position for each entity.
pub fn record(
    conn: &Connection,
    library_id: i64,
    entity: Entity,
    id: i64,
    op: Op,
    now: i64,
) -> Result<()> {
    conn.prepare_cached(
        "INSERT OR REPLACE INTO library_changes (library_id, entity_type, entity_id, op, \
         changed_at) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![library_id, Text(entity), id, Text(op), now])?;
    Ok(())
}
