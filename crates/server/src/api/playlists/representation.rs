//! A playlist as the API shows it.

use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::api::Id;
use crate::api::refs::ImageRef;
use crate::api::time::serialize_rfc3339;

/// The spec's `Playlist`. The mosaic and custom images are not served yet, so `image` is `null`
/// and `customImage` false; `personal` is left out.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    id: Id,
    library_id: Id,
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
    version: i64,
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
];

/// A playlist from a row of [`SELECT`].
pub fn playlist(row: &Row) -> rusqlite::Result<Playlist> {
    Ok(Playlist {
        id: Id(row.get(0)?),
        library_id: Id(row.get(1)?),
        title: row.get(2)?,
        description: row.get(3)?,
        image: None,
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
