//! How the API represents an album, and how that representation is read from the database.
//!
//! A list reads every row of a page by position from [`SELECT`], since looking a column up by
//! name costs several times as much. A single album reads the rest by name.

use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::api::Id;
use crate::api::refs::{self, Credit, ImageRef, TagRef};
use crate::api::sql::timestamp;

/// The spec's `AlbumSummary`, without `personal` until accounts exist.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummary {
    id: Id,
    title: Option<String>,
    artists: Vec<Credit>,
    release_date: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    genres: Vec<TagRef>,
    track_count: i64,
    track_total: Option<i64>,
    disc_count: i64,
    disc_total: Option<i64>,
    duration_us: i64,
    image: Option<ImageRef>,
    availability: &'static str,
    added_at: String,
}

/// The spec's `Album`.
#[derive(Serialize)]
pub struct Album {
    #[serde(flatten)]
    summary: AlbumSummary,
    labels: Vec<String>,
    discs: Vec<Disc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Disc {
    /// `None` for tracks with no disc tag, which the scanner files as disc 0.
    number: Option<i64>,
    track_count: i64,
    track_total: Option<i64>,
}

/// The columns [`summary`] reads, in order.
pub const SELECT: &[&str] = &[
    "al.id",
    "al.title",
    "al.release_date",
    "al.type",
    "al.track_count",
    // The spec's trackTotal sums the tagged totals of the discs present; NULL if none is tagged.
    "(SELECT sum(track_total) FROM album_discs WHERE album_discs.album_id = al.id)",
    "al.disc_count",
    "al.disc_total",
    "al.duration_us",
    "al.image_id",
    "al.available_track_count > 0",
    timestamp!("al.added_at"),
];

/// The whole album, or `None` if the library has no such album.
pub fn album(conn: &Connection, library: i64, album: i64) -> rusqlite::Result<Option<Album>> {
    let sql = format!(
        "SELECT {}, al.labels AS labels FROM albums al WHERE al.library_id = ?1 AND al.id = ?2",
        SELECT.join(", ")
    );
    let Some((summary, labels)) = conn
        .prepare_cached(&sql)?
        .query_row([library, album], |row| {
            Ok((summary(conn, row)?, row.get::<_, String>("labels")?))
        })
        .optional()?
    else {
        return Ok(None);
    };
    let discs = conn
        .prepare_cached(
            "SELECT disc_number, track_count, track_total FROM album_discs \
             WHERE album_id = ?1 ORDER BY disc_number",
        )?
        .query_map([album], |row| {
            Ok(Disc {
                number: Some(row.get(0)?).filter(|number| *number != 0),
                track_count: row.get(1)?,
                track_total: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some(Album {
        summary,
        // The schema checks that labels is a JSON array; the scanner writes strings.
        labels: serde_json::from_str(&labels).unwrap_or_default(),
        discs,
    }))
}

/// An album from a row of [`SELECT`], with its artists and genres.
pub fn summary(conn: &Connection, row: &Row) -> rusqlite::Result<AlbumSummary> {
    let id: i64 = row.get(0)?;
    Ok(AlbumSummary {
        id: Id(id),
        title: row.get(1)?,
        artists: refs::artists(conn, "album", id)?,
        release_date: row.get(2)?,
        kind: row.get(3)?,
        genres: refs::genres(conn, "album", id)?,
        track_count: row.get(4)?,
        track_total: row.get(5)?,
        disc_count: row.get(6)?,
        disc_total: row.get(7)?,
        duration_us: row.get(8)?,
        image: ImageRef::new(row.get(9)?),
        availability: if row.get(10)? { "available" } else { "missing" },
        added_at: row.get(11)?,
    })
}
