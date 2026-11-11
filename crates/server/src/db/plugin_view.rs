//! What plugins see of a library's tracks, albums and artists: the columns `get-tracks`,
//! `get-albums` and `get-artists` read (`crates/server/src/plugins/library.rs`), and a fingerprint
//! of that view and of what a plugin reaches from it, so the change feed can tell a change
//! plugins can see from one they can't (`0010_plugin_changes.sql`).
//!
//! Each view reads one table, and what it shows of others through subqueries that each read one
//! table by key, so a record on a scan batch's path looks rows up whatever the planner's
//! statistics say (`design/database.md` §4): a join can be planned as a scan of a table they
//! wrongly take to be tiny.

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension, Result, Row, params};
use sha2::{Digest, Sha256};

use super::feed::Entity;

/// One kind of entity as plugins read it by ID with [`super::by_ids`].
pub struct View {
    /// The table, with its alias.
    pub table: &'static str,
    /// The `WHERE` clause that follows it, its terms in parentheses so more can be added with
    /// `AND`: a row that fails it is not shown to plugins.
    pub rest: &'static str,
    /// The columns plugins are given, in the order their records read them.
    pub columns: &'static [&'static str],
    /// What else a plugin reaches from the entity, which a change to must wake it too.
    reached: &'static [&'static str],
}

/// A track, as `t`. Missing tracks are left out.
pub const TRACK: View = View {
    table: "tracks t",
    rest: "WHERE (t.missing_since IS NULL)",
    columns: &[
        "t.id",
        "t.title",
        "(SELECT json_group_array(artist_name) FROM (SELECT artist_name FROM track_artists \
          WHERE track_id = t.id AND artist_name IS NOT NULL ORDER BY position))",
        "t.album_id",
        "(SELECT title FROM albums WHERE id = t.album_id)",
        "t.disc_number",
        "t.track_number",
        "t.release_date",
        "t.isrc",
        "t.duration_us",
        "t.lyrics_kind <> 'none'",
        "t.root_id",
        "t.path",
    ],
    // A plugin can read the file and its sidecars.
    reached: &["t.file_size", "t.file_mtime", "t.sidecar_stamp"],
};

/// An album, as `al`. Only one with tracks is shown, as the browse lists show them.
pub const ALBUM: View = View {
    table: "albums al",
    rest: "WHERE (al.track_count > 0)",
    columns: &[
        "al.id",
        "al.title",
        "(SELECT json_group_array(name) FROM (SELECT (SELECT name FROM artists \
          WHERE id = aa.artist_id) AS name FROM album_artists aa WHERE aa.album_id = al.id \
          ORDER BY aa.position) WHERE name IS NOT NULL)",
        "al.release_date",
        "al.image_id IS NOT NULL",
    ],
    // Its artwork, and how many tracks it has, of which present, how long and on how many discs,
    // which move as tracks are added, removed or go missing; a track renumbered or swapped for
    // one as long moves none of them, and reaches plugins only as a track change.
    reached: &[
        "al.image_id",
        "al.track_count",
        "al.available_track_count",
        "al.duration_us",
        "al.disc_count",
    ],
};

/// An artist, as `ar`. Only one with music is shown.
pub const ARTIST: View = View {
    table: "artists ar",
    rest: "WHERE (ar.album_count > 0 OR ar.track_count > 0)",
    columns: &["ar.id", "ar.name", "ar.image_id IS NOT NULL", "coalesce(ar.biography, '') <> ''"],
    // Its image, its biography, and how many albums `artist-albums` lists, which moves as one is
    // added or emptied; one swapped for another reaches plugins only as album changes.
    reached: &["ar.image_id", "ar.biography", "ar.album_count"],
};

impl View {
    /// Everything the fingerprint covers: the columns, then what is reached from them.
    pub fn observed(&self) -> Vec<&'static str> {
        self.columns.iter().chain(self.reached).copied().collect()
    }
}

/// A fingerprint of what plugins see of `entity` `id` in `library`: 0 when they can't see it
/// (gone, missing, or empty), and none for a kind no hook reads. Equal fingerprints mean nothing
/// a plugin reads of it changed, short of an 8-byte hash collision.
pub fn fingerprint(
    conn: &Connection,
    library: i64,
    entity: Entity,
    id: i64,
) -> Result<Option<i64>> {
    let view = match entity {
        Entity::Track => &TRACK,
        Entity::Album => &ALBUM,
        Entity::Artist => &ARTIST,
        Entity::Library | Entity::Tag => return Ok(None),
    };
    let alias = view.table.rsplit_once(' ').map_or(view.table, |(_, alias)| alias);
    let sql = format!(
        "SELECT {} FROM {} {} AND {alias}.id = ?2 AND {alias}.library_id = ?1",
        view.observed().join(", "),
        view.table,
        view.rest
    );
    let found = conn.prepare_cached(&sql)?.query_row(params![library, id], digest).optional()?;
    Ok(Some(found.unwrap_or(0)))
}

/// The first 8 bytes of the SHA-256 of `row`'s cells, each as its type, then its length and
/// bytes where it has them, so no two different rows run together into the same input.
fn digest(row: &Row) -> Result<i64> {
    let mut hash = Sha256::new();
    for i in 0..row.as_ref().column_count() {
        match row.get_ref(i)? {
            ValueRef::Null => hash.update([0]),
            ValueRef::Integer(n) => {
                hash.update([1]);
                hash.update(n.to_le_bytes());
            }
            ValueRef::Real(x) => {
                hash.update([2]);
                hash.update(x.to_bits().to_le_bytes());
            }
            ValueRef::Text(bytes) => bytes_of(&mut hash, 3, bytes),
            ValueRef::Blob(bytes) => bytes_of(&mut hash, 4, bytes),
        }
    }
    let mut first = [0; 8];
    first.copy_from_slice(&hash.finalize()[..8]);
    Ok(i64::from_le_bytes(first))
}

/// Adds a text or blob cell to `hash`: its type `tag`, its length, then `bytes`.
fn bytes_of(hash: &mut Sha256, tag: u8, bytes: &[u8]) {
    hash.update([tag]);
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

#[cfg(test)]
mod tests;
