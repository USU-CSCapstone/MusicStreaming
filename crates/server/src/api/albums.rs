//! Albums: `listAlbums` and `getAlbum` (`api/openapi.yaml`).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::page::{self, Order, Page, Sort, Source, Unknown, timestamp};
use super::query::Query;
use super::refs::{Credit, ImageRef, TagRef};
use super::{Code, Id, Problem};
use crate::db::Database;

/// The spec's `AlbumSummary`, without `personal` until accounts exist.
#[derive(Serialize)]
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
const SELECT: &[&str] = &[
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

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AlbumSort {
    #[default]
    Name,
    Artist,
    DateAdded,
    ReleaseDate,
    Duration,
    // `playCount` and `lastPlayed` wait for listening history, and answer `422` until then.
}

impl AlbumSort {
    /// Each is one of `albums`' indexes after `library_id`.
    fn sort(self) -> &'static Sort {
        match self {
            AlbumSort::Name => &Sort {
                label: "albums.name",
                columns: &["al.sort_key", "al.id"],
                unknown: Unknown::Empty,
            },
            AlbumSort::Artist => &Sort {
                label: "albums.artist",
                columns: &["al.artist_sort_key", "al.sort_key", "al.id"],
                unknown: Unknown::Empty,
            },
            AlbumSort::DateAdded => &Sort {
                label: "albums.dateAdded",
                columns: &["al.added_at", "al.id"],
                unknown: Unknown::Never,
            },
            AlbumSort::ReleaseDate => &Sort {
                label: "albums.releaseDate",
                columns: &["al.release_date", "al.sort_key", "al.id"],
                unknown: Unknown::Null,
            },
            AlbumSort::Duration => &Sort {
                label: "albums.duration",
                columns: &["al.duration_us", "al.id"],
                unknown: Unknown::Never,
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

/// The parameters `listAlbums` supports so far. Any other, including the spec's other
/// filters, answers `422` rather than being ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    sort: AlbumSort,
    #[serde(default)]
    order: Order,
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
) -> Result<Json<Page<AlbumSummary>>, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    let sort = query.sort.sort();
    let after = sort.after(query.cursor.as_deref(), query.order)?;
    let limit = page::limit(query.limit)?;
    let filtered = query.artist_id.is_some();
    let source = source(library, query.artist_id.as_deref(), query.artist_credit);
    db.read(move |conn| {
        let Some(album_count) = page::library_count(conn, library, "album_count")? else {
            return Ok(None);
        };
        // The scanner keeps the library's count, so only a filtered list counts its rows.
        let total = if filtered {
            page::count(conn, &source)?
        } else {
            album_count
        };
        let (items, next_cursor) =
            page::fetch(conn, &source, sort, query.order, after, limit, |row| {
                summary(conn, row)
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
    Path((library_id, album_id)): Path<(String, String)>,
) -> Result<Json<Album>, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    let Id(album) = Id::parse(&album_id)?;
    db.read(move |conn| {
        let sql = format!(
            "SELECT {}, al.labels FROM albums al WHERE al.library_id = ?1 AND al.id = ?2",
            SELECT.join(", ")
        );
        let Some((summary, labels)) = conn
            .prepare_cached(&sql)?
            .query_row([library, album], |row| {
                Ok((summary(conn, row)?, row.get::<_, String>(SELECT.len())?))
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
    })
    .await?
    .map(Json)
    .ok_or_else(|| Problem::new(Code::NotFound))
}

/// Albums credited to this artist in the album artists.
const OWNED: &str = "SELECT album_id FROM album_artists WHERE artist_id = ?";
/// Albums with a track credited to this artist.
const ON_TRACKS: &str = "SELECT tracks.album_id FROM track_artists \
     JOIN tracks ON tracks.id = track_artists.track_id WHERE track_artists.artist_id = ?";

fn source(library: i64, artist: Option<&str>, credit: ArtistCredit) -> Source {
    let Some(artist) = artist else {
        return Source {
            select: SELECT,
            from: "albums al".to_owned(),
            filter: "al.library_id = ?".to_owned(),
            params: vec![library.into()],
        };
    };
    // A malformed artist ID matches no album, like an unknown one: IDs are never negative.
    let artist = Id::canonical(artist).map_or(-1, |Id(id)| id);
    let (credited, times) = match credit {
        ArtistCredit::Owned => (OWNED.to_owned(), 1),
        ArtistCredit::Featured => (format!("{ON_TRACKS} EXCEPT {OWNED}"), 2),
        ArtistCredit::Any => (format!("{OWNED} UNION {ON_TRACKS}"), 2),
    };
    let mut params = vec![artist.into(); times];
    params.push(library.into());
    Source {
        select: SELECT,
        // An artist's albums are few next to the library's, so the query starts from them and
        // sorts them, rather than walking the whole sort index; `CROSS JOIN` fixes that order.
        // The library filter still applies to every album it reaches.
        from: format!("({credited}) credited CROSS JOIN albums al ON al.id = credited.album_id"),
        filter: "al.library_id = ?".to_owned(),
        params,
    }
}

/// An album's artists, in order.
pub fn album_artists(conn: &Connection, album: i64) -> rusqlite::Result<Vec<Credit>> {
    conn.prepare_cached(
        "SELECT artists.id, artists.name FROM album_artists \
         JOIN artists ON artists.id = album_artists.artist_id \
         WHERE album_artists.album_id = ?1 ORDER BY album_artists.position",
    )?
    .query_map([album], |row| {
        Ok(Credit {
            id: Id(row.get(0)?),
            name: row.get(1)?,
        })
    })?
    .collect()
}

/// An album from a row of [`SELECT`], with its artists and genres.
fn summary(conn: &Connection, row: &Row) -> rusqlite::Result<AlbumSummary> {
    let id: i64 = row.get(0)?;
    Ok(AlbumSummary {
        id: Id(id),
        title: row.get(1)?,
        artists: album_artists(conn, id)?,
        release_date: row.get(2)?,
        kind: row.get(3)?,
        genres: conn
            .prepare_cached(
                "SELECT tags.id, tags.name FROM album_tags JOIN tags ON tags.id = album_tags.tag_id \
                 WHERE album_tags.album_id = ?1 ORDER BY tags.sort_key, tags.id",
            )?
            .query_map([id], |row| {
                Ok(TagRef {
                    id: Id(row.get(0)?),
                    name: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?,
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use axum::http::StatusCode;
    use rusqlite::Transaction;
    use serde_json::{Value, json};

    use super::super::tests::{app, send};
    use super::*;
    use crate::db::libraries;

    /// Library 1 holds, by name: "Abbey Road" (1969), "Kid A" (2000), "Nevermind" (no date),
    /// and the unknown album (no title, no date); library 2 holds one album of its own.
    fn fixture(tx: &Transaction) -> rusqlite::Result<()> {
        libraries::create(tx, Some(1), "Music", &[Path::new("/music")], &[])?;
        libraries::create(tx, Some(2), "Other", &[Path::new("/other")], &[])?;
        tx.execute_batch(
            "INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at, updated_at) VALUES
                 (10, 1, 'The Beatles', 'the beatles', CAST('beatles' AS BLOB), 0, 0),
                 (11, 1, 'Radiohead', 'radiohead', CAST('radiohead' AS BLOB), 0, 0),
                 (12, 1, 'Nirvana', 'nirvana', CAST('nirvana' AS BLOB), 0, 0),
                 (13, 1, NULL, '', x'', 0, 0),
                 (20, 2, 'Elsewhere', 'elsewhere', CAST('elsewhere' AS BLOB), 0, 0);
             INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key,
                                 artist_sort_key, release_date, disc_total, disc_count, track_count,
                                 available_track_count, duration_us, labels, added_at, updated_at) VALUES
                 (101, 1, 'Abbey Road', 'abbey road', 'the beatles', CAST('abbey road' AS BLOB),
                  CAST('beatles' AS BLOB), '1969-09-26', 1, 1, 17, 17, 2830000000, '[\"Apple\"]', 1000, 0),
                 (102, 1, 'Kid A', 'kid a', 'radiohead', CAST('kid a' AS BLOB),
                  CAST('radiohead' AS BLOB), '2000', NULL, 1, 10, 0, 2980000000, '[]', 4000, 0),
                 (103, 1, 'Nevermind', 'nevermind', 'nirvana', CAST('nevermind' AS BLOB),
                  CAST('nirvana' AS BLOB), NULL, NULL, 1, 12, 12, 2940000000, '[]', 2000, 0),
                 (104, 1, NULL, '', '', x'', x'', NULL, NULL, 1, 3, 3, 600000000, '[]', 3000, 0),
                 (201, 2, 'Elsewhere', 'elsewhere', 'elsewhere', CAST('elsewhere' AS BLOB),
                  CAST('elsewhere' AS BLOB), '2010', NULL, 1, 1, 1, 1, '[]', 0, 0);
             INSERT INTO album_artists (library_id, album_id, position, artist_id) VALUES
                 (1, 101, 0, 10), (1, 102, 0, 11), (1, 103, 0, 12), (1, 104, 0, 13), (2, 201, 0, 20);
             INSERT INTO album_discs (library_id, album_id, disc_number, track_count, track_total) VALUES
                 (1, 101, 1, 17, 17), (1, 104, 0, 3, NULL);
             INSERT INTO tags (id, library_id, type, name, name_key, sort_key) VALUES
                 (30, 1, 'genre', 'Rock', 'rock', CAST('rock' AS BLOB));
             INSERT INTO album_tags (library_id, album_id, tag_id) VALUES (1, 101, 30);
             UPDATE libraries SET album_count = 4 WHERE id = 1;",
        )?;
        // Radiohead also plays on a track of Abbey Road, so it appears there without owning it.
        let root: i64 = tx.query_row(
            "SELECT id FROM library_roots WHERE library_id = 1",
            [],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size, file_mtime,
                                 title, sort_key, artist_sort_key, album_sort_key, codec, container,
                                 lossless, sample_rate_hz, channels, duration_us, added_at, updated_at)
             VALUES (1001, 1, 101, ?1, 'a.flac', 1, 1, 'Something', x'', x'', x'', 'flac', 'flac',
                     1, 44100, 2, 1, 0, 0)",
            [root],
        )?;
        tx.execute(
            "INSERT INTO track_artists (library_id, track_id, position, artist_id)
             VALUES (1, 1001, 0, 10), (1, 1001, 1, 11)",
            [],
        )?;
        Ok(())
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

    /// Every album title in the list at `uri`, a page of `limit` at a time.
    async fn titles(app: &axum::Router, uri: &str, limit: u32) -> Vec<Value> {
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
            let items = page["items"].as_array().unwrap();
            assert!(items.len() <= limit as usize);
            titles.extend(items.iter().map(|album| album["title"].clone()));
            match page["nextCursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => return titles,
            }
        }
    }

    #[tokio::test]
    async fn pages_through_every_sort_with_unknowns_last() {
        let (_temp, app) = app_with_fixture().await;
        let base = "/api/v1/libraries/1/albums";
        for limit in [1, 2, 100] {
            assert_eq!(
                titles(&app, base, limit).await,
                json!(["Abbey Road", "Kid A", "Nevermind", null])
                    .as_array()
                    .unwrap()
                    .clone(),
                "name, limit {limit}"
            );
            assert_eq!(
                titles(&app, &format!("{base}?order=desc"), limit).await,
                json!(["Nevermind", "Kid A", "Abbey Road", null])
                    .as_array()
                    .unwrap()
                    .clone(),
                "name descending, limit {limit}"
            );
            // Undated albums come last, in name order; there the unknown album's empty name
            // sorts like any other value, since only a sort's first column has an unknown part.
            assert_eq!(
                titles(&app, &format!("{base}?sort=releaseDate&order=desc"), limit).await,
                json!(["Kid A", "Abbey Road", "Nevermind", null])
                    .as_array()
                    .unwrap()
                    .clone(),
                "newest first, limit {limit}"
            );
            assert_eq!(
                titles(&app, &format!("{base}?sort=releaseDate"), limit).await,
                json!(["Abbey Road", "Kid A", null, "Nevermind"])
                    .as_array()
                    .unwrap()
                    .clone(),
                "oldest first, limit {limit}"
            );
            assert_eq!(
                titles(&app, &format!("{base}?sort=dateAdded&order=desc"), limit).await,
                json!(["Kid A", null, "Nevermind", "Abbey Road"])
                    .as_array()
                    .unwrap()
                    .clone(),
                "newest additions first, limit {limit}"
            );
        }
        let (_, page) = json(&app, base).await;
        assert_eq!(page["total"], 4);
    }

    #[tokio::test]
    async fn filters_by_artist_credit() {
        let (_temp, app) = app_with_fixture().await;
        let base = "/api/v1/libraries/1/albums";
        for (query, expected, total) in [
            ("artistId=11&artistCredit=owned", json!(["Kid A"]), 1),
            (
                "artistId=11&artistCredit=featured",
                json!(["Abbey Road"]),
                1,
            ),
            ("artistId=11", json!(["Abbey Road", "Kid A"]), 2),
            ("artistId=20", json!([]), 0),
            ("artistId=nobody", json!([]), 0),
        ] {
            let (status, page) = json(&app, &format!("{base}?{query}")).await;
            assert_eq!(status, StatusCode::OK, "{query}");
            let titles: Vec<Value> = page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["title"].clone())
                .collect();
            assert_eq!(Value::from(titles), expected, "{query}");
            assert_eq!(page["total"], total, "{query}");
        }
    }

    #[tokio::test]
    async fn gets_an_album_with_its_discs() {
        let (_temp, app) = app_with_fixture().await;
        let (status, album) = json(&app, "/api/v1/libraries/1/albums/101").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            album,
            json!({
                "id": "101", "title": "Abbey Road",
                "artists": [{ "id": "10", "name": "The Beatles" }],
                "releaseDate": "1969-09-26", "type": "album",
                "genres": [{ "id": "30", "name": "Rock" }],
                "trackCount": 17, "trackTotal": 17, "discCount": 1, "discTotal": 1,
                "durationUs": 2_830_000_000_i64, "image": null, "availability": "available",
                "addedAt": "1970-01-01T00:00:01.000Z", "labels": ["Apple"],
                "discs": [{ "number": 1, "trackCount": 17, "trackTotal": 17 }],
            })
        );
        let (_, unknown) = json(&app, "/api/v1/libraries/1/albums/104").await;
        assert_eq!(unknown["title"], Value::Null);
        assert_eq!(unknown["artists"], json!([{ "id": "13", "name": null }]));
        assert_eq!(unknown["trackTotal"], Value::Null);
        assert_eq!(
            unknown["discs"],
            json!([{ "number": null, "trackCount": 3, "trackTotal": null }])
        );
        let (_, missing) = json(&app, "/api/v1/libraries/1/albums/102").await;
        assert_eq!(missing["availability"], "missing");
    }

    #[tokio::test]
    async fn another_librarys_album_is_not_found() {
        let (_temp, app) = app_with_fixture().await;
        for uri in [
            "/api/v1/libraries/1/albums/201",
            "/api/v1/libraries/3/albums/101",
            "/api/v1/libraries/3/albums",
        ] {
            let (status, problem) = json(&app, uri).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(problem["code"], "not_found", "{uri}");
        }
    }

    #[tokio::test]
    async fn bad_parameters_are_validation_problems() {
        let (_temp, app) = app_with_fixture().await;
        let base = "/api/v1/libraries/1/albums";
        let (_, page) = json(&app, &format!("{base}?limit=1")).await;
        let name_cursor = page["nextCursor"].as_str().unwrap().to_owned();
        for query in [
            "limit=0".to_owned(),
            "limit=1001".to_owned(),
            "limit=many".to_owned(),
            "sort=playCount".to_owned(),
            "order=sideways".to_owned(),
            "genre=30".to_owned(),
            "cursor=zz".to_owned(),
            format!("sort=dateAdded&cursor={name_cursor}"),
            format!("order=desc&cursor={name_cursor}"),
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
            libraries::create(tx, Some(1), "Music", &[], &[])?;
            // 20,000 albums: 5 per artist, a sixth undated, a few with unknown names.
            tx.execute_batch(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000)
                 INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key,
                                     artist_sort_key, release_date, duration_us, added_at, updated_at)
                 SELECT i, 1, 'Album ' || i, 'album ' || i, 'k' || i,
                        CASE WHEN i % 1000 = 0 THEN x''
                             ELSE CAST(printf('album %08x', abs(random()) % 4294967296) AS BLOB) END,
                        CASE WHEN i % 997 = 0 THEN x''
                             ELSE CAST(printf('artist %05d', i % 4000) AS BLOB) END,
                        CASE WHEN i % 6 = 0 THEN NULL ELSE printf('%04d', 1950 + i % 75) END,
                        (i % 3000) * 1000000, i, 0
                 FROM n;
                 ANALYZE;",
            )
        })
        .await
        .unwrap();
        db.read(|conn| {
            for sort in [
                AlbumSort::Name,
                AlbumSort::Artist,
                AlbumSort::DateAdded,
                AlbumSort::ReleaseDate,
                AlbumSort::Duration,
            ] {
                page::assert_indexed(conn, &source(1, None, ArtistCredit::Any), sort.sort())?;
            }
            Ok(())
        })
        .await
        .unwrap();
    }
}
