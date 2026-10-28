use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::testing::catalog::{self, page_through};
use super::super::testing::{app, json};
use super::*;
use crate::api::{AnyId, page};
use crate::db::testing::{Artist, library, scattered_key};

#[tokio::test]
async fn pages_through_every_sort_with_the_unknown_artist_last() {
    let (_temp, _db, app) = catalog::app().await;
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
        json!({ "id": "10", "name": "The Beatles", "image": { "id": "40", "placeholder": null },
                "albumCount": 1, "trackCount": 5, "addedAt": "1970-01-01T00:00:03.000Z" })
    );
}

#[tokio::test]
async fn names_sort_without_their_accents_and_ties_by_id() {
    let (_temp, db, app) = app("");
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        for (id, name) in [(1, "Zappa"), (2, "Édith Piaf"), (3, "Dylan"), (4, "Edith Piaf")] {
            Artist::new(id, 1, name).insert(tx)?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let base = "/api/v1/libraries/1/artists";
    for limit in [1, 100] {
        assert_eq!(
            page_through(&app, base, limit, "name").await,
            json!(["Dylan", "Édith Piaf", "Edith Piaf", "Zappa"]),
            "limit {limit}"
        );
        assert_eq!(
            page_through(&app, &format!("{base}?order=desc"), limit, "name").await,
            json!(["Zappa", "Edith Piaf", "Édith Piaf", "Dylan"]),
            "limit {limit}"
        );
    }
}

#[tokio::test]
async fn gets_an_artist_with_its_biography_and_genres() {
    let (_temp, _db, app) = catalog::app().await;
    let (status, artist) = json(&app, "/api/v1/libraries/1/artists/10").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        artist,
        json!({
            "id": "10", "name": "The Beatles", "image": { "id": "40", "placeholder": null },
            "albumCount": 1, "trackCount": 5, "addedAt": "1970-01-01T00:00:03.000Z",
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
    let (_temp, _db, app) = catalog::app().await;
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
    let (_temp, _db, app) = catalog::app().await;
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
        library(tx, 1, "Music", "/music")?;
        // 20,000 artists, a few with unknown names.
        for i in 1..=20_000 {
            let artist = Artist::new(i, 1, &format!("Artist {i}"));
            let sort_key = if i % 1000 == 0 { Vec::new() } else { scattered_key("artist", i) };
            Artist { sort_key, added_at: i, ..artist }.insert(tx)?;
        }
        // Tag 1 is on half the artists, too many to lead; the rest have one of 299 others.
        tx.execute_batch(
            "WITH RECURSIVE n (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 300) \
             INSERT INTO tags (id, library_id, type, name, name_key, sort_key) \
             SELECT i, 1, 'genre', 'Genre ' || i, 'genre ' || i, CAST('genre ' || i AS BLOB) FROM n; \
             INSERT INTO artist_tags (library_id, artist_id, tag_id, track_count) \
             SELECT 1, id, CASE WHEN id % 2 = 1 THEN 1 ELSE 2 + id % 299 END, 1 FROM artists;",
        )?;
        tx.execute_batch("ANALYZE")
    })
    .await
    .unwrap();
    db.read(|conn| {
        let tagged = TagFilter { ids: vec![AnyId(Some(1))], tag_match: TagMatch::Any };
        for sort in [ArtistSort::Name, ArtistSort::DateAdded] {
            page::assert_indexed(conn, &source(conn, 1, &TagFilter::default())?, sort.sort())?;
            let walked = source(conn, 1, &tagged)?;
            assert!(matches!(walked.total, page::Total::Stored(10_000)));
            page::assert_indexed(conn, &walked, sort.sort())?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

/// An artist carries the tags of the tracks that credit it, and any linked to it alone: The
/// Beatles have Rock and Calm from "Come Together", and Pop.
#[tokio::test]
async fn filters_by_the_tags_of_their_work() {
    let (_temp, _db, app) = catalog::app().await;
    for (query, expected) in [
        ("tag=31", json!(["The Beatles"])),
        ("tag=30,31&tagMatch=all", json!(["The Beatles"])),
        ("tag=31,32", json!(["The Beatles"])),
        ("tag=30,50", json!([])),
        ("tag=nope", json!([])),
    ] {
        let uri = format!("/api/v1/libraries/1/artists?{query}");
        assert_eq!(page_through(&app, &uri, 1, "name").await, expected, "{query}");
        let (_, page) = json(&app, &uri).await;
        assert_eq!(page["total"], expected.as_array().unwrap().len(), "{query}");
    }
    assert_eq!(json(&app, "/api/v1/libraries/2/artists?tag=31").await.1["total"], 0);
}
