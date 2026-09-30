use std::path::Path;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::testing::{app, json, send};
use super::*;
use crate::db::libraries;

/// The tracks fixture (`tracks::tests::fixture`), with library 2's track counted, and an
/// empty library 3.
async fn app_with_fixture() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app("");
    db.write(|tx| {
        tracks::tests::fixture(tx)?;
        tx.execute("UPDATE libraries SET track_count = 1 WHERE id = 2", [])?;
        libraries::create(tx, Some(3), "Empty", &[Path::new("/empty")], &[])?;
        Ok(())
    })
    .await
    .unwrap();
    (temp, db, app)
}

/// The item with `id` from a browse list, as the list shows it.
async fn listed(app: &Router, list: &str, id: &str) -> Value {
    let (_, page) = json(app, &format!("/api/v1/libraries/1/{list}?limit=1000")).await;
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap()
        .clone()
}

/// Each section's type and item IDs, in order.
fn sections(body: &Value) -> Vec<(String, Vec<String>)> {
    body["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|section| {
            let kind = section["type"].as_str().unwrap().to_owned();
            let key = kind.trim_end_matches('s').to_owned();
            let ids = section["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item[&key]["id"].as_str().unwrap().to_owned())
                .collect();
            (kind, ids)
        })
        .collect()
}

#[tokio::test]
async fn results_look_like_the_browse_lists() {
    let (_temp, _db, app) = app_with_fixture().await;
    let (status, body) = json(&app, "/api/v1/libraries/1/search?q=beatles").await;
    assert_eq!(status, StatusCode::OK);
    let artist = listed(&app, "artists", "10").await;
    assert_eq!(
        body,
        json!({
            "query": "beatles",
            "top": { "type": "artist", "artist": artist },
            "sections": [{ "type": "artists", "items": [{ "type": "artist", "artist": artist }], "total": 1 }],
            "nearMisses": [],
            "libraryEmpty": false,
        })
    );

    // A typo, and a track exactly as `listTracks` shows it.
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=come%20togther").await;
    assert_eq!(
        body["top"],
        json!({ "type": "track", "track": listed(&app, "tracks", "1001").await })
    );
}

#[tokio::test]
async fn sections_follow_the_core_and_the_filters() {
    let (_temp, _db, app) = app_with_fixture().await;
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=kid").await;
    assert_eq!(
        sections(&body),
        [("albums".to_owned(), vec!["102".to_owned()])]
    );
    let (_, body) = json(
        &app,
        "/api/v1/libraries/1/search?q=kid&types=tracks,playlists",
    )
    .await;
    assert_eq!(body["sections"], json!([]));
    assert_eq!(body["top"], Value::Null);
    // Equally exact everywhere: tracks, then albums, then artists.
    let (_, body) = json(
        &app,
        "/api/v1/libraries/2/search?q=elsewhere&sectionLimit=1",
    )
    .await;
    assert_eq!(
        sections(&body),
        [
            ("tracks".to_owned(), vec!["2001".to_owned()]),
            ("albums".to_owned(), vec!["201".to_owned()]),
            ("artists".to_owned(), vec!["20".to_owned()]),
        ]
    );
}

#[tokio::test]
async fn only_the_callers_library_is_searched() {
    let (_temp, _db, app) = app_with_fixture().await;
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=elsewhere").await;
    assert_eq!(body["sections"], json!([]));
    assert_eq!(body["libraryEmpty"], false);
    let (_, body) = json(&app, "/api/v1/libraries/3/search?q=elsewhere").await;
    assert_eq!(body["sections"], json!([]));
    assert_eq!(body["libraryEmpty"], true, "no music yet, not no match");
}

#[tokio::test]
async fn the_index_follows_the_library() {
    let (_temp, db, app) = app_with_fixture().await;
    let uri = "/api/v1/libraries/1/search?q=octopus";
    let (_, body) = json(&app, uri).await;
    assert_eq!(body["sections"], json!([]));
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size, file_mtime,
                                 title, sort_key, artist_sort_key, album_sort_key, codec,
                                 container, lossless, sample_rate_hz, channels, duration_us,
                                 added_at, updated_at)
             SELECT 1007, 1, 101, root_id, 'o.flac', 1, 0, 'Octopus''s Garden', x'6f', x'',
                    x'', 'flac', 'flac', 1, 44100, 2, 1, 0, 0 FROM tracks WHERE id = 1001;
             INSERT INTO library_changes (library_id, entity_type, entity_id, op, at)
             VALUES (1, 'track', 1007, 'upsert', 0);",
        )
    })
    .await
    .unwrap();
    // The search that notices the change answers from the old index while a new one builds.
    for _ in 0..100 {
        let (_, body) = json(&app, uri).await;
        if body["sections"] != json!([]) {
            assert_eq!(
                sections(&body),
                [("tracks".to_owned(), vec!["1007".to_owned()])]
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the index never caught up");
}

#[tokio::test]
async fn problems() {
    let (_temp, _db, app) = app_with_fixture().await;
    for (uri, status) in [
        ("/api/v1/libraries/9/search?q=a", StatusCode::NOT_FOUND),
        (
            "/api/v1/libraries/1/search",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "/api/v1/libraries/1/search?q=",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "/api/v1/libraries/1/search?q=a&sectionLimit=0",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "/api/v1/libraries/1/search?q=a&sectionLimit=51",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "/api/v1/libraries/1/search?q=a&types=songs",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        // Filters not built yet are refused rather than ignored.
        (
            "/api/v1/libraries/1/search?q=a&genre=rock",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let (actual, content_type, _) = send(app.clone(), "GET", uri).await;
        assert_eq!(actual, status, "{uri}");
        assert_eq!(
            content_type.as_deref(),
            Some("application/problem+json"),
            "{uri}"
        );
    }
}
