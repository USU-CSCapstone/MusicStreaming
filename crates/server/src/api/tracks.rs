//! Tracks: `listTracks` and `getTrack` (`api/openapi.yaml`).

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::albums::album_artists;
use super::page::{self, Order, Page, Sort, Source, Unknown, timestamp};
use super::query::Query;
use super::refs::{Credit, ImageRef, TagRef};
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

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ArtistCredit {
    #[default]
    Any,
    Owned,
    Featured,
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
    let after = sort.after(query.cursor.as_deref(), query.order)?;
    let limit = page::limit(query.limit)?;
    let filtered = query.album_id.is_some() || query.artist_id.is_some();
    let source = source(
        library,
        query.album_id.as_deref(),
        query.artist_id.as_deref(),
        query.artist_credit,
    );
    db.read(move |conn| {
        let Some(track_count) = page::library_count(conn, library, "track_count")? else {
            return Ok(None);
        };
        // The scanner keeps the library's count, so only a filtered list counts its rows.
        let total = if filtered {
            page::count(conn, &source)?
        } else {
            track_count
        };
        // An album's tracks share its reference, so each album is read once per page.
        let mut albums = HashMap::new();
        let (items, next_cursor) =
            page::fetch(conn, &source, sort, query.order, after, limit, |row| {
                list_summary(conn, row, &mut albums)
            })?;
        Ok(Some(Page {
            items,
            next_cursor,
            total,
        }))
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
    let mut params = Vec::new();
    let from = match artist {
        None => "tracks t".to_owned(),
        Some(artist) => {
            let (credited, times) = match credit {
                ArtistCredit::Owned => (OWNED.to_owned(), 1),
                ArtistCredit::Featured => (format!("{ON_TRACKS} EXCEPT {OWNED}"), 2),
                ArtistCredit::Any => (format!("{OWNED} UNION {ON_TRACKS}"), 2),
            };
            params.extend(vec![key(artist).into(); times]);
            // An artist's tracks are few next to the library's, so the query starts from them
            // and sorts them; `CROSS JOIN` fixes that order.
            format!("({credited}) credited CROSS JOIN tracks t ON t.id = credited.track_id")
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

/// A track from a row of [`SELECT`], with its artists, album, and genres. `albums` holds the
/// album references already read.
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
        artists: conn
            .prepare_cached(
                "SELECT artists.id, artists.name FROM track_artists \
                 JOIN artists ON artists.id = track_artists.artist_id \
                 WHERE track_artists.track_id = ?1 ORDER BY track_artists.position",
            )?
            .query_map([id], |row| {
                Ok(Credit {
                    id: Id(row.get(0)?),
                    name: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?,
        album,
        // The scanner files a track with no disc tag as disc 0.
        disc_number: Some(disc).filter(|disc| *disc != 0),
        track_number: row.get(4)?,
        duration_us: row.get(5)?,
        release_date: row.get(6)?,
        genres: conn
            .prepare_cached(
                "SELECT tags.id, tags.name FROM track_tags JOIN tags ON tags.id = track_tags.tag_id \
                 WHERE track_tags.track_id = ?1 ORDER BY tags.sort_key, tags.id",
            )?
            .query_map([id], |row| {
                Ok(TagRef {
                    id: Id(row.get(0)?),
                    name: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?,
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
        artists: album_artists(conn, album)?,
        image: ImageRef::new(image),
    })
}

#[cfg(test)]
pub(super) mod tests {
    use std::path::Path;

    use axum::http::StatusCode;
    use rusqlite::Transaction;
    use serde_json::{Value, json};

    use super::super::tests::{app, send};
    use super::*;
    use crate::db::libraries;

    /// Library 1: "Abbey Road" by The Beatles with discs 1 and 2, one disc-1 track unnumbered,
    /// and one track missing; "Kid A" by Radiohead with a track The Beatles feature on; and
    /// an untagged file on the unknown album. Library 2 holds one track of its own.
    pub(in crate::api) fn fixture(tx: &Transaction) -> rusqlite::Result<()> {
        libraries::create(tx, Some(1), "Music", &[Path::new("/music")], &[])?;
        libraries::create(tx, Some(2), "Other", &[Path::new("/other")], &[])?;
        let root = |library: i64| -> rusqlite::Result<i64> {
            tx.query_row(
                "SELECT id FROM library_roots WHERE library_id = ?1",
                [library],
                |r| r.get(0),
            )
        };
        let (music, other) = (root(1)?, root(2)?);
        tx.execute_batch(
            "INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at, updated_at) VALUES
                 (10, 1, 'The Beatles', 'the beatles', CAST('beatles' AS BLOB), 0, 0),
                 (11, 1, 'Radiohead', 'radiohead', CAST('radiohead' AS BLOB), 0, 0),
                 (13, 1, NULL, '', x'', 0, 0),
                 (20, 2, 'Elsewhere', 'elsewhere', CAST('elsewhere' AS BLOB), 0, 0);
             INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key,
                                 artist_sort_key, loudness_lufs, peak_dbtp, added_at, updated_at) VALUES
                 (101, 1, 'Abbey Road', 'abbey road', 'the beatles', CAST('abbey road' AS BLOB),
                  CAST('beatles' AS BLOB), -9.5, -0.3, 0, 0),
                 (102, 1, 'Kid A', 'kid a', 'radiohead', CAST('kid a' AS BLOB),
                  CAST('radiohead' AS BLOB), NULL, NULL, 0, 0),
                 (104, 1, NULL, '', '', x'', x'', NULL, NULL, 0, 0),
                 (201, 2, 'Elsewhere', 'elsewhere', 'elsewhere', CAST('elsewhere' AS BLOB),
                  CAST('elsewhere' AS BLOB), NULL, NULL, 0, 0);
             INSERT INTO album_artists (library_id, album_id, position, artist_id) VALUES
                 (1, 101, 0, 10), (1, 102, 0, 11), (1, 104, 0, 13), (2, 201, 0, 20);
             INSERT INTO tags (id, library_id, type, name, name_key, sort_key) VALUES
                 (30, 1, 'genre', 'Rock', 'rock', CAST('rock' AS BLOB));",
        )?;
        let track = "INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size,
                         file_mtime, title, sort_key, artist_sort_key, album_sort_key, disc_number,
                         track_number, release_date, missing_since, codec, container, lossless,
                         sample_rate_hz, bit_depth, bitrate_kbps, channels, duration_us, added_at,
                         updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 1000, 0, ?6, CAST(lower(?6) AS BLOB), ?7, ?8, ?9,
                             ?10, ?11, ?12, 'flac', 'flac', 1, 44100, 16, 900, 2, 1000000, ?1, 0)";
        let beatles = rusqlite::types::Value::Blob(b"beatles".to_vec());
        let radiohead = rusqlite::types::Value::Blob(b"radiohead".to_vec());
        let empty = rusqlite::types::Value::Blob(Vec::new());
        let abbey = rusqlite::types::Value::Blob(b"abbey road".to_vec());
        let kid_a = rusqlite::types::Value::Blob(b"kid a".to_vec());
        let null = rusqlite::types::Value::Null;
        use rusqlite::types::Value::{Integer, Text};
        for row in [
            // id, library, album, root, path, title, artist key, album key, disc, number, date, missing
            vec![
                Integer(1001),
                Integer(1),
                Integer(101),
                Integer(music),
                Text("1.flac".into()),
                Text("Come Together".into()),
                beatles.clone(),
                abbey.clone(),
                Integer(1),
                Integer(1),
                Text("1969".into()),
                null.clone(),
            ],
            vec![
                Integer(1002),
                Integer(1),
                Integer(101),
                Integer(music),
                Text("2.flac".into()),
                Text("Something".into()),
                beatles.clone(),
                abbey.clone(),
                Integer(1),
                Integer(2),
                Text("1969".into()),
                Integer(5),
            ],
            vec![
                Integer(1003),
                Integer(1),
                Integer(101),
                Integer(music),
                Text("x.flac".into()),
                Text("Her Majesty".into()),
                beatles.clone(),
                abbey.clone(),
                Integer(1),
                null.clone(),
                Text("1969".into()),
                null.clone(),
            ],
            vec![
                Integer(1004),
                Integer(1),
                Integer(101),
                Integer(music),
                Text("d2.flac".into()),
                Text("Bonus".into()),
                beatles.clone(),
                abbey.clone(),
                Integer(2),
                Integer(1),
                Text("1969".into()),
                null.clone(),
            ],
            vec![
                Integer(1005),
                Integer(1),
                Integer(102),
                Integer(music),
                Text("k.flac".into()),
                Text("Idioteque".into()),
                radiohead.clone(),
                kid_a.clone(),
                Integer(1),
                Integer(8),
                Text("2000".into()),
                null.clone(),
            ],
            vec![
                Integer(1006),
                Integer(1),
                Integer(104),
                Integer(music),
                Text("u.flac".into()),
                Text("untitled".into()),
                empty.clone(),
                empty.clone(),
                Integer(0),
                null.clone(),
                null.clone(),
                null.clone(),
            ],
            vec![
                Integer(2001),
                Integer(2),
                Integer(201),
                Integer(other),
                Text("e.flac".into()),
                Text("Elsewhere".into()),
                empty.clone(),
                empty.clone(),
                Integer(0),
                null.clone(),
                null.clone(),
                null.clone(),
            ],
        ] {
            tx.execute(track, rusqlite::params_from_iter(row))?;
        }
        tx.execute_batch(
            "INSERT INTO track_artists (library_id, track_id, position, artist_id) VALUES
                 (1, 1001, 0, 10), (1, 1002, 0, 10), (1, 1003, 0, 10), (1, 1004, 0, 10),
                 (1, 1005, 0, 11), (1, 1005, 1, 10), (1, 1006, 0, 13), (2, 2001, 0, 20);
             INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) VALUES (1, 1001, 30, 'rock');
             UPDATE tracks SET explicit = 1, isrc = 'GBAYE0601690', lyrics_kind = 'synced',
                               loudness_lufs = -10.25, peak_dbtp = -0.5,
                               identifiers = '{\"musicbrainz_recordingid\":\"abc\"}' WHERE id = 1001;
             UPDATE libraries SET track_count = 6 WHERE id = 1;",
        )
    }

    async fn app_with_fixture() -> (tempfile::TempDir, axum::Router) {
        let (temp, db, app) = app("");
        db.write(fixture).await.unwrap();
        (temp, app)
    }

    async fn json(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
        let (status, _, body) = send(app.clone(), "GET", uri).await;
        (status, serde_json::from_slice(&body).unwrap())
    }

    /// Every track title in the list at `uri`, a page of `limit` at a time.
    async fn titles(app: &axum::Router, uri: &str, limit: u32) -> Value {
        let mut titles = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let separator = if uri.contains('?') { '&' } else { '?' };
            let mut page_uri = format!("{uri}{separator}limit={limit}");
            if let Some(cursor) = &cursor {
                page_uri += &format!("&cursor={cursor}");
            }
            let (status, page) = json(app, &page_uri).await;
            assert_eq!(status, StatusCode::OK, "{page}");
            titles.extend(
                page["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t["title"].clone()),
            );
            match page["nextCursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => return titles.into(),
            }
        }
    }

    #[tokio::test]
    async fn album_order_is_disc_then_number_with_unnumbered_tracks_last() {
        let (_temp, app) = app_with_fixture().await;
        let base = "/api/v1/libraries/1/tracks?sort=album";
        for limit in [1, 2, 100] {
            assert_eq!(
                titles(&app, base, limit).await,
                json!([
                    "Come Together",
                    "Something",
                    "Her Majesty",
                    "Bonus",
                    "Idioteque",
                    "untitled"
                ]),
                "limit {limit}"
            );
            assert_eq!(
                titles(&app, &format!("{base}&albumId=101"), limit).await,
                json!(["Come Together", "Something", "Her Majesty", "Bonus"]),
                "limit {limit}"
            );
            assert_eq!(
                titles(
                    &app,
                    "/api/v1/libraries/1/tracks?sort=releaseDate&order=desc",
                    limit
                )
                .await,
                json!([
                    "Idioteque",
                    "Bonus",
                    "Her Majesty",
                    "Something",
                    "Come Together",
                    "untitled"
                ]),
                "limit {limit}"
            );
            assert_eq!(
                titles(&app, "/api/v1/libraries/1/tracks", limit).await,
                json!([
                    "Bonus",
                    "Come Together",
                    "Her Majesty",
                    "Idioteque",
                    "Something",
                    "untitled"
                ]),
                "limit {limit}"
            );
        }
        let (_, page) = json(&app, "/api/v1/libraries/1/tracks").await;
        assert_eq!(page["total"], 6);
        let (_, page) = json(&app, &format!("{base}&albumId=101")).await;
        assert_eq!(page["total"], 4);
    }

    #[tokio::test]
    async fn filters_by_artist_credit() {
        let (_temp, app) = app_with_fixture().await;
        let base = "/api/v1/libraries/1/tracks?sort=album";
        for (query, expected) in [
            (
                "artistId=10&artistCredit=owned",
                json!(["Come Together", "Something", "Her Majesty", "Bonus"]),
            ),
            ("artistId=10&artistCredit=featured", json!(["Idioteque"])),
            (
                "artistId=10",
                json!([
                    "Come Together",
                    "Something",
                    "Her Majesty",
                    "Bonus",
                    "Idioteque"
                ]),
            ),
            ("artistId=10&albumId=102", json!(["Idioteque"])),
            ("artistId=20", json!([])),
            ("albumId=nope", json!([])),
        ] {
            assert_eq!(
                titles(&app, &format!("{base}&{query}"), 2).await,
                expected,
                "{query}"
            );
        }
    }

    #[tokio::test]
    async fn summaries_and_full_tracks() {
        let (_temp, app) = app_with_fixture().await;
        let summary = json!({
            "id": "1001", "title": "Come Together",
            "artists": [{ "id": "10", "name": "The Beatles" }],
            "album": { "id": "101", "title": "Abbey Road",
                       "artists": [{ "id": "10", "name": "The Beatles" }], "image": null },
            "discNumber": 1, "trackNumber": 1, "durationUs": 1_000_000, "releaseDate": "1969",
            "genres": [{ "id": "30", "name": "Rock" }], "explicit": true,
            "audio": { "codec": "flac", "lossless": true }, "availability": "available",
            "addedAt": "1970-01-01T00:00:01.001Z",
        });
        let (_, page) = json(&app, "/api/v1/libraries/1/tracks?albumId=101&sort=album").await;
        assert_eq!(page["items"][0], summary);
        assert_eq!(page["items"][1]["availability"], "missing");
        assert_eq!(page["items"][2]["trackNumber"], Value::Null);

        let (status, track) = json(&app, "/api/v1/libraries/1/tracks/1001").await;
        assert_eq!(status, StatusCode::OK);
        let mut expected = summary;
        expected["audio"] = json!({
            "codec": "flac", "lossless": true, "container": "flac", "bitrateKbps": 900,
            "sampleRateHz": 44100, "bitDepth": 16, "channels": 2, "fileSizeBytes": 1000,
        });
        expected["lyrics"] = json!("synced");
        expected["loudness"] = json!({ "trackLufs": -10.25, "trackPeakDbtp": -0.5,
                                       "albumLufs": -9.5, "albumPeakDbtp": -0.3 });
        expected["identifiers"] =
            json!({ "isrc": "GBAYE0601690", "musicbrainz_recordingid": "abc" });
        assert_eq!(track, expected);

        let (_, untagged) = json(&app, "/api/v1/libraries/1/tracks/1006").await;
        assert_eq!(untagged["discNumber"], Value::Null);
        assert_eq!(untagged["album"]["title"], Value::Null);
        assert_eq!(untagged["loudness"], Value::Null);
        assert_eq!(untagged["identifiers"], json!({ "isrc": null }));
    }

    #[tokio::test]
    async fn another_librarys_track_is_not_found() {
        let (_temp, app) = app_with_fixture().await;
        for uri in [
            "/api/v1/libraries/1/tracks/2001",
            "/api/v1/libraries/3/tracks/1001",
            "/api/v1/libraries/3/tracks",
            "/api/v1/libraries/1/tracks/-1",
        ] {
            let (status, problem) = json(&app, uri).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(problem["code"], "not_found", "{uri}");
        }
    }

    #[tokio::test]
    async fn bad_parameters_are_validation_problems() {
        let (_temp, app) = app_with_fixture().await;
        let base = "/api/v1/libraries/1/tracks";
        let (_, page) = json(&app, &format!("{base}?sort=album&limit=1")).await;
        let album_cursor = page["nextCursor"].as_str().unwrap().to_owned();
        for query in [
            "limit=1001".to_owned(),
            "sort=top".to_owned(),
            "sort=duration".to_owned(),
            "genre=30".to_owned(),
            format!("sort=releaseDate&cursor={album_cursor}"),
            format!("cursor={album_cursor}"),
        ] {
            let (status, problem) = json(&app, &format!("{base}?{query}")).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
            assert_eq!(problem["code"], "validation_failed", "{query}");
        }
    }

    #[tokio::test]
    async fn every_sort_reads_from_an_index() {
        let (_temp, db, _app) = app("");
        db.write(|tx| {
            libraries::create(tx, Some(1), "Music", &[Path::new("/music")], &[])?;
            // 20,000 tracks on 2,000 albums: a sixth undated, some unnumbered, a few with
            // unknown names.
            tx.execute_batch(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000)
                 INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key,
                                     artist_sort_key, added_at, updated_at)
                 SELECT i, 1, 'Album ' || i, 'album ' || i, 'k' || i,
                        CASE WHEN i % 200 = 0 THEN x''
                             ELSE CAST(printf('album %08x', abs(random()) % 4294967296) AS BLOB) END,
                        x'', i, 0
                 FROM n;
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000)
                 INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size, file_mtime,
                                     title, sort_key, artist_sort_key, album_sort_key, disc_number,
                                     track_number, release_date, codec, container, lossless,
                                     sample_rate_hz, channels, duration_us, added_at, updated_at)
                 SELECT i, 1, 1 + i % 2000, (SELECT id FROM library_roots), 'p' || i, 1, 0,
                        'Track ' || i,
                        CAST(printf('track %08x', abs(random()) % 4294967296) AS BLOB),
                        CASE WHEN i % 997 = 0 THEN x''
                             ELSE CAST(printf('artist %05d', i % 3000) AS BLOB) END,
                        (SELECT sort_key FROM albums WHERE id = 1 + i % 2000),
                        1 + i % 2, CASE WHEN i % 13 = 0 THEN NULL ELSE i % 12 END,
                        CASE WHEN i % 6 = 0 THEN NULL ELSE printf('%04d', 1950 + i % 75) END,
                        'flac', 'flac', 1, 44100, 2, 1, i, 0
                 FROM n;
                 ANALYZE;",
            )
        })
        .await
        .unwrap();
        db.read(|conn| {
            for sort in [
                TrackSort::Name,
                TrackSort::Artist,
                TrackSort::Album,
                TrackSort::DateAdded,
                TrackSort::ReleaseDate,
            ] {
                let source = source(1, None, None, ArtistCredit::Any);
                page::assert_indexed(conn, &source, sort.sort())?;
            }
            let one_album = source(1, Some("7"), None, ArtistCredit::Any);
            page::assert_indexed(conn, &one_album, &WITHIN_ALBUM)?;
            Ok(())
        })
        .await
        .unwrap();
    }
}
