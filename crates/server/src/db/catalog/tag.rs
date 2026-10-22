//! A tag's name and sort key, from the spellings its tracks carry (`design/database.md` §3).

use jewelcase_core::sort_key;
use rusqlite::{OptionalExtension, Result, Transaction, params};

use crate::db::feed::{self, Entity, Op};

/// The tag's displayed name and sort key from the spellings its tracks count, or its deletion
/// when no track carries it. Whether one does is read from the tracks' genres, not the count.
pub(super) fn refresh_tag(tx: &Transaction<'_>, lib: i64, tag: i64, now: i64) -> Result<()> {
    let Some((carried, name)) = tx
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM track_tags WHERE tag_id = ?1), \
             (SELECT name FROM tag_spellings WHERE tag_id = ?1 ORDER BY uses DESC, name LIMIT 1) \
             FROM tags WHERE id = ?1",
            [tag],
            |r| Ok((r.get::<_, bool>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    if !carried {
        tx.execute("DELETE FROM tags WHERE id = ?1", [tag])?;
        return feed::record(tx, lib, Entity::Tag, tag, Op::Delete, now);
    }
    let name = name.unwrap_or_default();
    tx.execute(
        "UPDATE tags SET name = ?2, sort_key = ?3 WHERE id = ?1",
        params![tag, name, sort_key(&name, None).into_bytes()],
    )?;
    feed::record(tx, lib, Entity::Tag, tag, Op::Upsert, now)
}
