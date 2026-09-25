//! The library change feed (`design/database.md` §5): one row per entity,
//! coalesced on write, in the same transaction as the change.

use rusqlite::{Connection, params};

use super::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entity {
    Library,
    Track,
    Album,
    Artist,
    Tag,
}

impl Entity {
    fn as_str(self) -> &'static str {
        match self {
            Entity::Library => "library",
            Entity::Track => "track",
            Entity::Album => "album",
            Entity::Artist => "artist",
            Entity::Tag => "tag",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    conn.execute(
        "DELETE FROM library_changes WHERE library_id = ?1 AND entity_type = ?2 AND entity_id = ?3",
        params![library_id, entity.as_str(), id],
    )?;
    conn.execute(
        "INSERT INTO library_changes (library_id, entity_type, entity_id, op, at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            library_id,
            entity.as_str(),
            id,
            match op {
                Op::Upsert => "upsert",
                Op::Delete => "delete",
            },
            now
        ],
    )?;
    Ok(())
}
