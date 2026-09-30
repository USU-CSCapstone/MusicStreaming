use std::path::Path;

use axum::http::StatusCode;
use rusqlite::Transaction;
use serde_json::{Value, json};

use super::super::testing::{app, json, page_through};
use super::*;
use crate::api::page;
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

#[tokio::test]
async fn pages_through_every_sort_with_unknowns_last() {
    let (_temp, app) = app_with_fixture().await;
    let base = "/api/v1/libraries/1/albums";
    for limit in [1, 2, 100] {
        assert_eq!(
            page_through(&app, base, limit, "title").await,
            json!(["Abbey Road", "Kid A", "Nevermind", null]),
            "name, limit {limit}"
        );
        assert_eq!(
            page_through(&app, &format!("{base}?order=desc"), limit, "title").await,
            json!(["Nevermind", "Kid A", "Abbey Road", null]),
            "name descending, limit {limit}"
        );
        // Undated albums come last, in name order; there the unknown album's empty name
        // sorts like any other value, since only a sort's first column has an unknown part.
        assert_eq!(
            page_through(
                &app,
                &format!("{base}?sort=releaseDate&order=desc"),
                limit,
                "title"
            )
            .await,
            json!(["Kid A", "Abbey Road", "Nevermind", null]),
            "newest first, limit {limit}"
        );
        assert_eq!(
            page_through(&app, &format!("{base}?sort=releaseDate"), limit, "title").await,
            json!(["Abbey Road", "Kid A", null, "Nevermind"]),
            "oldest first, limit {limit}"
        );
        assert_eq!(
            page_through(
                &app,
                &format!("{base}?sort=dateAdded&order=desc"),
                limit,
                "title"
            )
            .await,
            json!(["Kid A", null, "Nevermind", "Abbey Road"]),
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
