//! A playlist as the API shows it.

use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::api::Id;
use crate::api::refs::ImageRef;
use crate::api::time::serialize_rfc3339;

/// The spec's `Playlist`. `image` is the generated artwork (`artwork`), `customImage` is false
/// as custom images are not served yet, and `personal` is left out.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub(super) id: Id,
    pub(super) library_id: Id,
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
    pub(super) version: i64,
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
];

/// A playlist from a row of [`SELECT`].
pub fn playlist(row: &Row) -> rusqlite::Result<Playlist> {
    Ok(Playlist {
        id: Id(row.get(0)?),
        library_id: Id(row.get(1)?),
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
    conn.prepare_cached(&format!(
        "SELECT {} FROM playlists p WHERE p.id = ?1 AND p.user_id = ?2 AND p.library_id = ?3",
        SELECT.join(", ")
    ))?
    .query_row(rusqlite::params![id, user, library], playlist)
    .optional()
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
