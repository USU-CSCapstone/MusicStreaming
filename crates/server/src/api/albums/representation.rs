//! How the API represents an album, and how that representation is read from the database.
//!
//! A list reads every row of a page by position from [`SELECT`], since looking a column up by
//! name costs several times as much. A single album reads the rest by name.

use std::num::NonZeroI64;

use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::api::Id;
use crate::api::refs::{
    Credit, ImageRef, TagsByType, artists_json, availability, placeholder, tags_json,
};
use crate::api::time::serialize_rfc3339;
use crate::db::Json;

/// The spec's `AlbumSummary`, without `personal`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummary {
    id: Id,
    title: Option<String>,
    artists: Vec<Credit>,
    release_date: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    tags: TagsByType,
    track_count: i64,
    track_total: Option<i64>,
    disc_count: i64,
    disc_total: Option<i64>,
    duration_us: i64,
    image: Option<ImageRef>,
    availability: &'static str,
    #[serde(serialize_with = "serialize_rfc3339")]
    added_at: i64,
}

/// The spec's `Album`.
#[derive(Serialize)]
pub struct Album {
    #[serde(flatten)]
    summary: AlbumSummary,
    labels: Vec<String>,
    discs: Vec<Disc>,
}

/// One entry of the spec's `Album.discs`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Disc {
    /// `None` for tracks with no disc tag, which the scanner files as disc 0.
    number: Option<NonZeroI64>,
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
    "al.track_total",
    "al.disc_count",
    "al.disc_total",
    "al.duration_us",
    "al.image_id",
    "al.available_track_count > 0",
    "al.added_at",
    artists_json!("album", "al.id"),
    tags_json!("album", "al.id"),
    placeholder!("al.image_id"),
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
            Ok((summary(row)?, row.get::<_, Json<Vec<String>>>("labels")?.0))
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
                number: NonZeroI64::new(row.get(0)?),
                track_count: row.get(1)?,
                track_total: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some(Album { summary, labels, discs }))
}

/// An album from a row of [`SELECT`], with its artists and tags.
pub fn summary(row: &Row) -> rusqlite::Result<AlbumSummary> {
    let id: i64 = row.get(0)?;
    Ok(AlbumSummary {
        id: Id(id),
        title: row.get(1)?,
        artists: row.get::<_, Json<_>>(12)?.0,
        release_date: row.get(2)?,
        kind: row.get(3)?,
        tags: row.get::<_, Json<_>>(13)?.0,
        track_count: row.get(4)?,
        track_total: row.get(5)?,
        disc_count: row.get(6)?,
        disc_total: row.get(7)?,
        duration_us: row.get(8)?,
        image: ImageRef::new(row.get(9)?, row.get(14)?),
        availability: availability(row.get(10)?),
        added_at: row.get(11)?,
    })
}
