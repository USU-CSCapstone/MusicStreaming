//! A playlist as the API shows it.

use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::refs::ImageRef;
use crate::time::serialize_rfc3339;
use crate::{Id, hidden};

/// The spec's `Playlist`. `image` is the generated artwork (`artwork`), `customImage` is false
/// as custom images are not served yet, and `personal` is left out.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: Id,
    pub library_id: Id,
    /// `liked` for the user's Liked Songs, `saved` for the rest (`requirements/playlists.md` §11).
    pub kind: String,
    title: String,
    description: Option<String>,
    image: Option<ImageRef>,
    custom_image: bool,
    track_count: i64,
    duration_us: i64,
    pinned: bool,
    #[serde(serialize_with = "serialize_rfc3339")]
    created_at: i64,
    #[serde(serialize_with = "serialize_rfc3339")]
    modified_at: i64,
    pub version: i64,
}

impl Playlist {
    /// The playlist as its user sees it, less its items of tracks they hid, `(count, duration)`
    /// (`requirements/conventions.md` §8).
    pub(super) fn lose(&mut self, (count, duration): (i64, i64)) {
        self.track_count -= count;
        self.duration_us -= duration;
    }
}

/// The columns [`playlist`] reads, in order, from `playlists p`.
pub const SELECT: &[&str] = &[
    "p.id",
    "p.library_id",
    "p.title",
    "p.description",
    "p.pinned",
    "p.created_at",
    "p.modified_at",
    "p.track_count",
    "p.duration_us",
    "p.version",
    "p.image_key",
    // Only a single image has a placeholder; a mosaic's key starts with a letter.
    "CASE WHEN p.image_key GLOB '[0-9]*' THEN (SELECT im.placeholder FROM images im \
     WHERE im.library_id = p.library_id AND im.id = CAST(p.image_key AS INTEGER)) END",
    "p.kind",
];

/// A playlist from a row of [`SELECT`].
pub fn playlist(row: &Row) -> rusqlite::Result<Playlist> {
    Ok(Playlist {
        id: Id(row.get(0)?),
        library_id: Id(row.get(1)?),
        kind: row.get(12)?,
        title: row.get(2)?,
        description: row.get(3)?,
        image: ImageRef::named(row.get(10)?, row.get(11)?),
        custom_image: false,
        pinned: row.get(4)?,
        created_at: row.get(5)?,
        modified_at: row.get(6)?,
        track_count: row.get(7)?,
        duration_us: row.get(8)?,
        version: row.get(9)?,
    })
}

/// Playlist `id`, if it is `user`'s own in `library`.
pub fn get(
    conn: &Connection,
    user: i64,
    library: i64,
    id: i64,
) -> rusqlite::Result<Option<Playlist>> {
    let mut found = conn
        .prepare_cached(&format!(
            "SELECT {} FROM playlists p WHERE p.id = ?1 AND p.user_id = ?2 AND p.library_id = ?3",
            SELECT.join(", ")
        ))?
        .query_row(rusqlite::params![id, user, library], playlist)
        .optional()?;
    if let Some(playlist) = &mut found
        && let Some(&lost) = hidden::playlists(conn, user, library)?.get(&id)
    {
        playlist.lose(lost);
    }
    Ok(found)
}

/// The version and item count of `user`'s playlist `id` in `library`, if it is theirs.
pub fn header(
    conn: &Connection,
    user: i64,
    library: i64,
    id: i64,
) -> rusqlite::Result<Option<(i64, i64)>> {
    conn.prepare_cached(
        "SELECT version, track_count FROM playlists \
         WHERE id = ?1 AND user_id = ?2 AND library_id = ?3",
    )?
    .query_row(rusqlite::params![id, user, library], |row| Ok((row.get(0)?, row.get(1)?)))
    .optional()
}
