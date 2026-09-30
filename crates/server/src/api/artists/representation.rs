//! How the API represents an artist, and how that representation is read from the database.
//!
//! A list reads every row of a page by position from [`SELECT`], since looking a column up by
//! name costs several times as much. A single artist reads the rest by name.

use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::api::Id;
use crate::api::refs::{ImageRef, TagRef, genres_json, placeholder};
use crate::api::sql::{Json, timestamp};

/// The spec's `ArtistSummary`, without `personal` until accounts exist.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistSummary {
    id: Id,
    /// `None` for the unknown artist.
    name: Option<String>,
    image: Option<ImageRef>,
    /// Albums the artist owns.
    album_count: i64,
    /// Owned and featured tracks.
    track_count: i64,
    added_at: String,
}

/// The spec's `Artist`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    #[serde(flatten)]
    summary: ArtistSummary,
    biography: Option<String>,
    genres: Vec<TagRef>,
    appearance_count: i64,
}

/// The columns [`summary`] reads, in order.
pub const SELECT: &[&str] = &[
    "ar.id",
    "ar.name",
    "ar.image_id",
    "ar.album_count",
    "ar.track_count",
    timestamp!("ar.added_at"),
    placeholder!("ar.image_id"),
];

/// The whole artist, or `None` if the library has no such artist.
pub fn artist(conn: &Connection, library: i64, artist: i64) -> rusqlite::Result<Option<Artist>> {
    let sql = format!(
        "SELECT {}, ar.biography AS biography, ar.appearance_count AS appearance_count, \
         {} AS genres FROM artists ar WHERE ar.library_id = ?1 AND ar.id = ?2",
        SELECT.join(", "),
        genres_json!("artist", "ar.id"),
    );
    conn.prepare_cached(&sql)?
        .query_row([library, artist], |row| {
            Ok(Artist {
                summary: summary(row)?,
                biography: row.get("biography")?,
                genres: row.get::<_, Json<_>>("genres")?.0,
                appearance_count: row.get("appearance_count")?,
            })
        })
        .optional()
}

/// An artist from a row of [`SELECT`].
pub fn summary(row: &Row) -> rusqlite::Result<ArtistSummary> {
    Ok(ArtistSummary {
        id: Id(row.get(0)?),
        name: row.get(1)?,
        image: ImageRef::new(row.get(2)?, row.get(6)?),
        album_count: row.get(3)?,
        track_count: row.get(4)?,
        added_at: row.get(5)?,
    })
}
