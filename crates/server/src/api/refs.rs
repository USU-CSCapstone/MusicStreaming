//! Small references to other entities, shared by the browse responses.

use rusqlite::Connection;
use serde::Serialize;

use super::Id;

/// The spec's `ArtistCredit`.
#[derive(Clone, Serialize)]
pub struct Credit {
    pub id: Id,
    /// `None` for the unknown artist.
    pub name: Option<String>,
}

/// The spec's `TagRef`.
#[derive(Clone, Serialize)]
pub struct TagRef {
    pub id: Id,
    pub name: String,
}

/// The spec's `ImageRef`.
#[derive(Clone, Serialize)]
pub struct ImageRef {
    id: Id,
    /// A draft field; empty until the image job makes placeholders (`images.placeholder`).
    placeholder: &'static str,
}

impl ImageRef {
    pub fn new(image_id: Option<i64>) -> Option<ImageRef> {
        image_id.map(|id| ImageRef {
            id: Id(id),
            placeholder: "",
        })
    }
}

/// The artists credited on an `album` or a `track`, in credit order, from its
/// `{owner}_artists` table.
pub fn artists(conn: &Connection, owner: &'static str, id: i64) -> rusqlite::Result<Vec<Credit>> {
    conn.prepare_cached(&format!(
        "SELECT artists.id, artists.name FROM {owner}_artists credit \
         JOIN artists ON artists.id = credit.artist_id \
         WHERE credit.{owner}_id = ?1 ORDER BY credit.position"
    ))?
    .query_map([id], |row| {
        Ok(Credit {
            id: Id(row.get(0)?),
            name: row.get(1)?,
        })
    })?
    .collect()
}

/// The genres of an `album`, an `artist`, or a `track`, in name order, from its
/// `{owner}_tags` table.
pub fn genres(conn: &Connection, owner: &'static str, id: i64) -> rusqlite::Result<Vec<TagRef>> {
    conn.prepare_cached(&format!(
        "SELECT tags.id, tags.name FROM {owner}_tags tagged JOIN tags ON tags.id = tagged.tag_id \
         WHERE tagged.{owner}_id = ?1 ORDER BY tags.sort_key, tags.id"
    ))?
    .query_map([id], |row| {
        Ok(TagRef {
            id: Id(row.get(0)?),
            name: row.get(1)?,
        })
    })?
    .collect()
}
