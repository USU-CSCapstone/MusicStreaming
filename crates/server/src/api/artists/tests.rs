use std::path::Path;

use axum::http::StatusCode;
use rusqlite::Transaction;
use serde_json::{Value, json};

use super::super::testing::{app, json, page_through};
use super::*;
use crate::api::page;
use crate::db::libraries;

/// Library 1 holds Radiohead, The Beatles (with a photo, biography, and genre), Nirvana,
/// and the unknown artist; library 2 holds one artist of its own.
fn fixture(tx: &Transaction) -> rusqlite::Result<()> {
    libraries::create(tx, Some(1), "Music", &[Path::new("/music")], &[])?;
    libraries::create(tx, Some(2), "Other", &[Path::new("/other")], &[])?;
    let root: i64 =
        tx.query_row("SELECT id FROM library_roots WHERE library_id = 1", [], |r| r.get(0))?;
    tx.execute(
        "INSERT INTO images (id, library_id, hash, format, width, height, root_id, path, embedded)
         VALUES (40, 1, x'00', 'jpeg', 600, 600, ?1, 'The Beatles/artist.jpg', 0)",
        [root],
    )?;
    tx.execute_batch(
        "INSERT INTO artists (id, library_id, name, name_key, sort_key, image_id, biography,
                              album_count, track_count, appearance_count, added_at, updated_at) VALUES
             (10, 1, 'The Beatles', 'the beatles', CAST('beatles' AS BLOB), 40,
              'Four lads from Liverpool.', 2, 30, 1, 3000, 0),
             (11, 1, 'Radiohead', 'radiohead', CAST('radiohead' AS BLOB), NULL, NULL,
              1, 11, 0, 1000, 0),
             (12, 1, 'Nirvana', 'nirvana', CAST('nirvana' AS BLOB), NULL, NULL, 1, 12, 0, 4000, 0),
             (13, 1, NULL, '', x'', NULL, NULL, 1, 3, 0, 2000, 0),
             (20, 2, 'Elsewhere', 'elsewhere', CAST('elsewhere' AS BLOB), NULL, NULL,
              1, 1, 0, 0, 0);
         INSERT INTO tags (id, library_id, type, name, name_key, sort_key) VALUES
             (30, 1, 'genre', 'Rock', 'rock', CAST('rock' AS BLOB)),
             (31, 1, 'genre', 'Pop', 'pop', CAST('pop' AS BLOB));
         INSERT INTO artist_tags (library_id, artist_id, tag_id) VALUES (1, 10, 30), (1, 10, 31);
         UPDATE libraries SET artist_count = 4 WHERE id = 1;",
    )
}

async fn app_with_fixture() -> (tempfile::TempDir, axum::Router) {
    let (temp, db, app) = app("");
    db.write(fixture).await.unwrap();
    (temp, app)
}

#[tokio::test]
async fn pages_through_every_sort_with_the_unknown_artist_last() {
    let (_temp, app) = app_with_fixture().await;
    let base = "/api/v1/libraries/1/artists";
    for limit in [1, 3, 100] {
        // "The Beatles" files under B.
        assert_eq!(
            page_through(&app, base, limit, "name").await,
            json!(["The Beatles", "Nirvana", "Radiohead", null]),
            "limit {limit}"
        );
        assert_eq!(
            page_through(&app, &format!("{base}?order=desc"), limit, "name").await,
            json!(["Radiohead", "Nirvana", "The Beatles", null]),
            "limit {limit}"
        );
        assert_eq!(
            page_through(&app, &format!("{base}?sort=dateAdded&order=desc"), limit, "name").await,
            json!(["Nirvana", "The Beatles", null, "Radiohead"]),
            "limit {limit}"
        );
    }
    let (_, page) = json(&app, base).await;
    assert_eq!(page["total"], 4);
    assert_eq!(
        page["items"][0],
        json!({ "id": "10", "name": "The Beatles", "image": { "id": "40", "placeholder": "" },
                "albumCount": 2, "trackCount": 30, "addedAt": "1970-01-01T00:00:03.000Z" })
    );
}

#[tokio::test]
async fn gets_an_artist_with_its_biography_and_genres() {
    let (_temp, app) = app_with_fixture().await;
    let (status, artist) = json(&app, "/api/v1/libraries/1/artists/10").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        artist,
        json!({
            "id": "10", "name": "The Beatles", "image": { "id": "40", "placeholder": "" },
            "albumCount": 2, "trackCount": 30, "addedAt": "1970-01-01T00:00:03.000Z",
            "biography": "Four lads from Liverpool.",
            "genres": [{ "id": "31", "name": "Pop" }, { "id": "30", "name": "Rock" }],
            "appearanceCount": 1,
        })
    );
    let (_, unknown) = json(&app, "/api/v1/libraries/1/artists/13").await;
    assert_eq!(unknown["name"], Value::Null);
    assert_eq!(unknown["biography"], Value::Null);
    assert_eq!(unknown["genres"], json!([]));
}

#[tokio::test]
async fn another_librarys_artist_is_not_found() {
    let (_temp, app) = app_with_fixture().await;
    for uri in [
        "/api/v1/libraries/1/artists/20",
        "/api/v1/libraries/3/artists/10",
        "/api/v1/libraries/3/artists",
        "/api/v1/libraries/1/artists/x",
    ] {
        let (status, problem) = json(&app, uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(problem["code"], "not_found", "{uri}");
    }
}

#[tokio::test]
async fn bad_parameters_are_validation_problems() {
    let (_temp, app) = app_with_fixture().await;
    let base = "/api/v1/libraries/1/artists";
    let (_, page) = json(&app, &format!("{base}?limit=1")).await;
    let name_cursor = page["nextCursor"].as_str().unwrap().to_owned();
    // Shaped exactly like an artist cursor, but from the album list.
    let album_cursor = crate::api::cursor::encode(
        "albums.name.asc",
        &[rusqlite::types::Value::Blob(b"beatles".to_vec()), rusqlite::types::Value::Integer(10)],
    );
    for query in [
        "limit=0".to_owned(),
        "sort=releaseDate".to_owned(),
        "sort=playCount".to_owned(),
        "availability=missing".to_owned(),
        format!("sort=dateAdded&cursor={name_cursor}"),
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
        libraries::create(tx, Some(1), "Music", &[], &[])?;
        // 20,000 artists, a few with unknown names.
        tx.execute_batch(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000)
             INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at, updated_at)
             SELECT i, 1, 'Artist ' || i, 'artist ' || i,
                    CASE WHEN i % 1000 = 0 THEN x''
                         ELSE CAST(printf('artist %08x', abs(random()) % 4294967296) AS BLOB) END,
                    i, 0
             FROM n;
             ANALYZE;",
        )
    })
    .await
    .unwrap();
    db.read(|conn| {
        for sort in [ArtistSort::Name, ArtistSort::DateAdded] {
            page::assert_indexed(conn, &source(1), sort.sort())?;
        }
        Ok(())
    })
    .await
    .unwrap();
}
