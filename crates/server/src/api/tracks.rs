//! Tracks: `listTracks` and `getTrack` (`api/openapi.yaml`).

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::credit::ArtistCredit;
use super::page::{Order, Page, Request, Sort, Source, Unknown};
use super::query::Query;
use super::refs::{self, Credit, ImageRef, TagRef};
use super::sql::timestamp;
use super::{Code, Id, Problem};
use crate::db::Database;

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

/// The columns [`loudness`] reads, in order, from `tracks t` joined to its album `al`.
pub const LOUDNESS: &str = "t.loudness_lufs, t.peak_dbtp, al.loudness_lufs, al.peak_dbtp";

/// The track's loudness, from the [`LOUDNESS`] columns starting at `first`. Null until analysis
/// measures the track (`requirements/playback.md` §5); a track too quiet to measure stays null
/// too.
pub fn loudness(row: &Row, first: usize) -> rusqlite::Result<Option<Loudness>> {
    Ok(match (row.get(first)?, row.get(first + 1)?) {
        (Some(track_lufs), Some(track_peak_dbtp)) => Some(Loudness {
            track_lufs,
            track_peak_dbtp,
            album_lufs: row.get(first + 2)?,
            album_peak_dbtp: row.get(first + 3)?,
        }),
        _ => None,
    })
}

/// The columns [`summary`] reads, in order.
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
];

/// A track with no number sorts last on its disc. This matches the indexes' expression
/// (`0003_catalog.sql`), which keeps NULL out of the keyset.
const TRACK_NUMBER: &str = "ifnull(t.track_number, 9223372036854775807)";

/// Album order within one album: disc, then number. `tracks_by_album_order` holds exactly this
/// after `album_id`, where the library-wide album order would sort the album on every page.
const WITHIN_ALBUM: Sort = Sort {
    label: "tracks.album.withinAlbum",
    columns: &["t.disc_number", TRACK_NUMBER, "t.id"],
    unknown: Unknown::Never,
};

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum TrackSort {
    #[default]
    Name,
    Artist,
    Album,
    DateAdded,
    ReleaseDate,
    // `playCount`, `lastPlayed`, and `top` wait for listening history, and answer `422` until
    // then.
}

impl TrackSort {
    /// Each is one of `tracks`' indexes after `library_id`.
    fn sort(self) -> &'static Sort {
        match self {
            TrackSort::Name => &Sort {
                label: "tracks.name",
                columns: &["t.sort_key", "t.id"],
                unknown: Unknown::Empty,
            },
            TrackSort::Artist => &Sort {
                label: "tracks.artist",
                columns: &["t.artist_sort_key", "t.sort_key", "t.id"],
                unknown: Unknown::Empty,
            },
            TrackSort::Album => &Sort {
                label: "tracks.album",
                columns: &[
                    "t.album_sort_key",
                    "t.album_id",
                    "t.disc_number",
                    TRACK_NUMBER,
                    "t.id",
                ],
                unknown: Unknown::Empty,
            },
            TrackSort::DateAdded => &Sort {
                label: "tracks.dateAdded",
                columns: &["t.added_at", "t.id"],
                unknown: Unknown::Never,
            },
            TrackSort::ReleaseDate => &Sort {
                label: "tracks.releaseDate",
                columns: &[
                    "t.release_date",
                    "t.album_sort_key",
                    "t.album_id",
                    "t.disc_number",
                    TRACK_NUMBER,
                    "t.id",
                ],
                unknown: Unknown::Null,
            },
        }
    }
}

/// The parameters `listTracks` supports so far. Any other, including the spec's other
/// filters, answers `422` rather than being ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    sort: TrackSort,
    #[serde(default)]
    order: Order,
    album_id: Option<String>,
    artist_id: Option<String>,
    #[serde(default)]
    artist_credit: ArtistCredit,
    cursor: Option<String>,
    limit: Option<u32>,
}

pub async fn list(
    State(db): State<Arc<Database>>,
    Path(library_id): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<TrackSummary>>, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    // An album's own track listing, the common case, reads straight from its index.
    let sort = match (query.sort, &query.album_id) {
        (TrackSort::Album, Some(_)) => &WITHIN_ALBUM,
        (sort, _) => sort.sort(),
    };
    let request = Request::new(sort, query.order, query.cursor, query.limit)?;
    let filtered = query.album_id.is_some() || query.artist_id.is_some();
    let source = source(
        library,
        query.album_id.as_deref(),
        query.artist_id.as_deref(),
        query.artist_credit,
    );
    db.read(move |conn| {
        // An album's tracks share its reference, so each album is read once per page.
        let mut albums = HashMap::new();
        request.read(conn, library, "track_count", filtered, &source, |row| {
            list_summary(conn, row, &mut albums)
        })
    })
    .await?
    .map(Json)
    .ok_or_else(|| Problem::new(Code::NotFound))
}

pub async fn get(
    State(db): State<Arc<Database>>,
    Path((library_id, track_id)): Path<(String, String)>,
) -> Result<Json<Track>, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    let Id(track) = Id::parse(&track_id)?;
    db.read(move |conn| {
        let sql = format!(
            "SELECT {}, t.container, t.bitrate_kbps, t.sample_rate_hz, t.bit_depth, t.channels, \
             t.file_size, t.lyrics_kind, {LOUDNESS}, t.isrc, t.identifiers \
             FROM tracks t JOIN albums al ON al.id = t.album_id \
             WHERE t.library_id = ?1 AND t.id = ?2",
            SELECT.join(", ")
        );
        conn.prepare_cached(&sql)?
            .query_row([library, track], |row| {
                let extra = SELECT.len();
                let audio = AudioProperties {
                    summary: AudioSummary {
                        codec: row.get(8)?,
                        lossless: row.get(9)?,
                    },
                    container: row.get(extra)?,
                    bitrate_kbps: row.get(extra + 1)?,
                    sample_rate_hz: row.get(extra + 2)?,
                    bit_depth: row.get(extra + 3)?,
                    channels: row.get(extra + 4)?,
                    file_size_bytes: row.get(extra + 5)?,
                };
                // `isrc`, then every other identifier tag as read (`requirements/tracks.md` §6).
                // The schema checks that identifiers is a JSON object.
                let mut identifiers: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(&row.get::<_, String>(extra + 12)?).unwrap_or_default();
                identifiers.insert(
                    "isrc".to_owned(),
                    row.get::<_, Option<String>>(extra + 11)?.into(),
                );
                Ok(Track {
                    summary: summary(conn, row, &mut HashMap::new(), audio)?,
                    lyrics: row.get(extra + 6)?,
                    loudness: loudness(row, extra + 7)?,
                    identifiers,
                })
            })
            .optional()
    })
    .await?
    .map(Json)
    .ok_or_else(|| Problem::new(Code::NotFound))
}

/// Tracks whose album this artist is an album artist of.
const OWNED: &str = "SELECT tracks.id AS track_id FROM album_artists \
     JOIN tracks ON tracks.album_id = album_artists.album_id WHERE album_artists.artist_id = ?";
/// Tracks credited to this artist.
const ON_TRACKS: &str = "SELECT track_id FROM track_artists WHERE artist_id = ?";

fn source(library: i64, album: Option<&str>, artist: Option<&str>, credit: ArtistCredit) -> Source {
    // A malformed ID matches nothing, like an unknown one: IDs are never negative.
    let key = |text: &str| Id::canonical(text).map_or(-1, |Id(id)| id);
    let (from, mut params) = match artist {
        None => ("tracks t".to_owned(), Vec::new()),
        Some(artist) => {
            let (credited, params) = credit.query(OWNED, ON_TRACKS, artist);
            // An artist's tracks are few next to the library's, so the query starts from them
            // and sorts them; `CROSS JOIN` fixes that order.
            let from =
                format!("({credited}) credited CROSS JOIN tracks t ON t.id = credited.track_id");
            (from, params)
        }
    };
    let mut filter = "t.library_id = ?".to_owned();
    params.push(library.into());
    if let Some(album) = album {
        filter += " AND t.album_id = ?";
        params.push(key(album).into());
    }
    Source {
        select: SELECT,
        from,
        filter,
        params,
    }
}

/// A track as lists show it, from a row of [`SELECT`]. `albums` holds the album references read
/// so far, since an album's tracks share one.
pub fn list_summary(
    conn: &Connection,
    row: &Row,
    albums: &mut HashMap<i64, AlbumRef>,
) -> rusqlite::Result<TrackSummary> {
    let audio = AudioSummary {
        codec: row.get(8)?,
        lossless: row.get(9)?,
    };
    summary(conn, row, albums, audio)
}

fn summary<A>(
    conn: &Connection,
    row: &Row,
    albums: &mut HashMap<i64, AlbumRef>,
    audio: A,
) -> rusqlite::Result<TrackSummary<A>> {
    let id: i64 = row.get(0)?;
    let album_id: i64 = row.get(2)?;
    let album = match albums.get(&album_id) {
        Some(album) => album.clone(),
        None => {
            let album = album_ref(conn, album_id)?;
            albums.insert(album_id, album.clone());
            album
        }
    };
    let disc: i64 = row.get(3)?;
    Ok(TrackSummary {
        id: Id(id),
        title: row.get(1)?,
        artists: refs::artists(conn, "track", id)?,
        album,
        // The scanner files a track with no disc tag as disc 0.
        disc_number: Some(disc).filter(|disc| *disc != 0),
        track_number: row.get(4)?,
        duration_us: row.get(5)?,
        release_date: row.get(6)?,
        genres: refs::genres(conn, "track", id)?,
        explicit: row.get(7)?,
        audio,
        availability: if row.get(10)? { "available" } else { "missing" },
        added_at: row.get(11)?,
    })
}

fn album_ref(conn: &Connection, album: i64) -> rusqlite::Result<AlbumRef> {
    let (title, image) = conn
        .prepare_cached("SELECT title, image_id FROM albums WHERE id = ?1")?
        .query_row([album], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(AlbumRef {
        id: Id(album),
        title,
        artists: refs::artists(conn, "album", album)?,
        image: ImageRef::new(image),
    })
}

#[cfg(test)]
pub(super) mod tests;
