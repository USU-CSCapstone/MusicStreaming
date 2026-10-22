//! How the API represents a track, and how that representation is read from the database.
//!
//! A list reads every row of a page by position from [`SELECT`], since looking a column up by
//! name costs several times as much. A single track reads the rest by name, with each column
//! named by `AS`: SQLite leaves unnamed columns' names open.

use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::api::Id;
use crate::api::refs::{Credit, ImageRef, TagRef, artists_json, genres_json, placeholder};
use crate::api::sql::timestamp;
use crate::db::Json;

/// The spec's `TrackSummary`, without `personal` until accounts exist. A full `Track` has the
/// same fields with more `audio`, so the audio type is a parameter.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackSummary<A = AudioSummary> {
    id: Id,
    title: String,
    artists: Vec<Credit>,
    album: AlbumRef,
    disc_number: Option<i64>,
    track_number: Option<i64>,
    duration_us: i64,
    release_date: Option<String>,
    genres: Vec<TagRef>,
    explicit: bool,
    audio: A,
    availability: &'static str,
    added_at: String,
    sort_keys: TrackSortKeys,
}

/// The spec's `TrackSortKeys`: what the server orders tracks by, so a client sorting a list it
/// holds orders it the same way (`requirements/conventions.md` §4).
#[derive(Clone, Serialize)]
pub struct TrackSortKeys {
    title: String,
    artist: String,
    album: String,
}

/// The spec's `Track`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    #[serde(flatten)]
    summary: TrackSummary<AudioProperties>,
    lyrics: String,
    loudness: Option<Loudness>,
    identifiers: serde_json::Map<String, serde_json::Value>,
}

/// The spec's `AlbumRef`.
#[derive(Clone, Serialize)]
pub struct AlbumRef {
    id: Id,
    title: Option<String>,
    artists: Vec<Credit>,
    image: Option<ImageRef>,
}

/// The spec's `AudioSummary`.
#[derive(Clone, Serialize)]
pub struct AudioSummary {
    codec: String,
    lossless: bool,
}

/// The spec's `AudioProperties`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioProperties {
    #[serde(flatten)]
    summary: AudioSummary,
    container: String,
    bitrate_kbps: Option<i64>,
    sample_rate_hz: i64,
    bit_depth: Option<i64>,
    channels: i64,
    file_size_bytes: i64,
}

/// The spec's `Loudness`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Loudness {
    track_lufs: f64,
    track_peak_dbtp: f64,
    album_lufs: Option<f64>,
    album_peak_dbtp: Option<f64>,
}

/// The columns [`loudness`] reads, from `tracks t` joined to its album `al`.
pub const LOUDNESS: &str = "t.loudness_lufs AS track_lufs, t.peak_dbtp AS track_peak_dbtp, \
     al.loudness_lufs AS album_lufs, al.peak_dbtp AS album_peak_dbtp";

/// The track's loudness, from the [`LOUDNESS`] columns. Null until analysis measures the track
/// (`requirements/playback.md` §5); a track too quiet to measure stays null too.
pub fn loudness(row: &Row) -> rusqlite::Result<Option<Loudness>> {
    Ok(match (row.get("track_lufs")?, row.get("track_peak_dbtp")?) {
        (Some(track_lufs), Some(track_peak_dbtp)) => Some(Loudness {
            track_lufs,
            track_peak_dbtp,
            album_lufs: row.get("album_lufs")?,
            album_peak_dbtp: row.get("album_peak_dbtp")?,
        }),
        _ => None,
    })
}

/// The columns [`list_summary`] reads, in order, from `tracks t` and [`JOINS`].
pub const SELECT: &[&str] = &[
    "t.id",
    "t.title",
    "t.album_id",
    "t.disc_number",
    "t.track_number",
    "t.duration_us",
    "t.release_date",
    "t.explicit",
    "t.codec",
    "t.lossless",
    "t.missing_since IS NULL",
    timestamp!("t.added_at"),
    artists_json!("track", "t.id"),
    genres_json!("track", "t.id"),
    "al.title",
    "al.image_id",
    artists_json!("album", "t.album_id"),
    placeholder!("al.image_id"),
    "t.sort_key",
    "t.artist_sort_key",
    "t.album_sort_key",
];

/// The track's album `al`, which [`SELECT`] reads once per row. A left join, so the track stays
/// the outer loop.
pub const JOINS: &str = "LEFT JOIN albums al ON al.id = t.album_id";

/// A track as lists show it, from a row of [`SELECT`].
pub fn list_summary(row: &Row) -> rusqlite::Result<TrackSummary> {
    summary(row, audio_summary(row)?)
}

/// The whole track, or `None` if the library has no such track.
pub fn track(conn: &Connection, library: i64, track: i64) -> rusqlite::Result<Option<Track>> {
    let sql = format!(
        "SELECT {}, t.container AS container, t.bitrate_kbps AS bitrate_kbps, \
         t.sample_rate_hz AS sample_rate_hz, t.bit_depth AS bit_depth, t.channels AS channels, \
         t.file_size AS file_size, t.lyrics_kind AS lyrics_kind, {LOUDNESS}, t.isrc AS isrc, \
         t.identifiers AS identifiers FROM tracks t {JOINS} WHERE t.library_id = ?1 AND t.id = ?2",
        SELECT.join(", ")
    );
    conn.prepare_cached(&sql)?
        .query_row([library, track], |row| {
            let audio = AudioProperties {
                summary: audio_summary(row)?,
                container: row.get("container")?,
                bitrate_kbps: row.get("bitrate_kbps")?,
                sample_rate_hz: row.get("sample_rate_hz")?,
                bit_depth: row.get("bit_depth")?,
                channels: row.get("channels")?,
                file_size_bytes: row.get("file_size")?,
            };
            // `isrc`, then every other identifier tag as read (`requirements/tracks.md` §6).
            let Json(mut identifiers) =
                row.get::<_, Json<serde_json::Map<String, serde_json::Value>>>("identifiers")?;
            identifiers.insert("isrc".to_owned(), row.get::<_, Option<String>>("isrc")?.into());
            Ok(Track {
                summary: summary(row, audio)?,
                lyrics: row.get("lyrics_kind")?,
                loudness: loudness(row)?,
                identifiers,
            })
        })
        .optional()
}

fn audio_summary(row: &Row) -> rusqlite::Result<AudioSummary> {
    Ok(AudioSummary { codec: row.get(8)?, lossless: row.get(9)? })
}

fn summary<A>(row: &Row, audio: A) -> rusqlite::Result<TrackSummary<A>> {
    let disc: i64 = row.get(3)?;
    Ok(TrackSummary {
        id: Id(row.get(0)?),
        title: row.get(1)?,
        artists: row.get::<_, Json<_>>(12)?.0,
        album: AlbumRef {
            id: Id(row.get(2)?),
            title: row.get(14)?,
            artists: row.get::<_, Json<_>>(16)?.0,
            image: ImageRef::new(row.get(15)?, row.get(17)?),
        },
        // The scanner files a track with no disc tag as disc 0.
        disc_number: Some(disc).filter(|disc| *disc != 0),
        track_number: row.get(4)?,
        duration_us: row.get(5)?,
        release_date: row.get(6)?,
        genres: row.get::<_, Json<_>>(13)?.0,
        explicit: row.get(7)?,
        audio,
        availability: if row.get(10)? { "available" } else { "missing" },
        added_at: row.get(11)?,
        sort_keys: TrackSortKeys {
            title: key(row, 18)?,
            artist: key(row, 19)?,
            album: key(row, 20)?,
        },
    })
}

/// A sort key, stored as the UTF-8 bytes `jewelcase_core::sort` built it from.
fn key(row: &Row, index: usize) -> rusqlite::Result<String> {
    String::from_utf8(row.get(index)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Blob, e.into())
    })
}
