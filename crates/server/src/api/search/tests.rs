use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::testing::{catalog, eventually, json, send};
use super::*;
use crate::db::testing::{Album, Artist, Track, library};

/// [`catalog::app`], plus an empty library 3.
async fn app_with_catalog() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = catalog::app().await;
    db.write(|tx| library(tx, 3, "Empty", "/empty")).await.unwrap();
    (temp, db, app)
}

/// The item with `id` from a browse list, as the list shows it.
async fn listed(app: &Router, list: &str, id: &str) -> Value {
    let (_, page) = json(app, &format!("/api/v1/libraries/1/{list}?limit=1000")).await;
    page["items"].as_array().unwrap().iter().find(|item| item["id"] == id).unwrap().clone()
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
    let (_temp, _db, app) = app_with_catalog().await;
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
    let (_temp, _db, app) = app_with_catalog().await;
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=kid").await;
    assert_eq!(sections(&body), [("albums".to_owned(), vec!["102".to_owned()])]);
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=kid&types=tracks,playlists").await;
    assert_eq!(body["sections"], json!([]));
    assert_eq!(body["top"], Value::Null);
    // Equally exact everywhere: tracks, then albums, then artists.
    let (_, body) = json(&app, "/api/v1/libraries/2/search?q=elsewhere&sectionLimit=1").await;
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
    let (_temp, _db, app) = app_with_catalog().await;
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=elsewhere").await;
    assert_eq!(body["sections"], json!([]));
    assert_eq!(body["libraryEmpty"], false);
    let (_, body) = json(&app, "/api/v1/libraries/3/search?q=elsewhere").await;
    assert_eq!(body["sections"], json!([]));
    assert_eq!(body["libraryEmpty"], true, "no music yet, not no match");
}

#[tokio::test]
async fn a_name_in_both_libraries_is_counted_once_in_each() {
    let (_temp, db, app) = app_with_catalog().await;
    // Library 1 gets a track, an album, and an artist named as library 2's are.
    db.write(|tx| {
        let title = "Elsewhere".to_owned();
        Track { title: title.clone(), ..Track::new(1007, 1, 101) }.insert(tx)?;
        Album { title: Some(title), ..Album::new(105, 1) }.insert(tx)?;
        Artist::new(14, 1, "Elsewhere").insert(tx)
    })
    .await
    .unwrap();
    let ids = |id: &str| vec![id.to_owned()];
    for (library, track, album, artist) in [(1, "1007", "105", "14"), (2, "2001", "201", "20")] {
        let uri = format!(
            "/api/v1/libraries/{library}/search?q=elsewhere&types=tracks,albums,artists&sectionLimit=1"
        );
        let (_, body) = json(&app, &uri).await;
        let expected = [
            ("tracks".to_owned(), ids(track)),
            ("albums".to_owned(), ids(album)),
            ("artists".to_owned(), ids(artist)),
        ];
        assert_eq!(sections(&body), expected, "library {library}");
        for section in body["sections"].as_array().unwrap() {
            assert_eq!(section["total"], 1, "library {library}: {section}");
        }
    }
}

#[tokio::test]
async fn what_is_gone_since_the_index_was_built_is_left_out() {
    let (_temp, db, app) = app_with_catalog().await;
    let uri = "/api/v1/libraries/1/search?q=untitled";
    let (_, body) = json(&app, uri).await;
    assert_eq!(sections(&body), [("tracks".to_owned(), vec!["1006".to_owned()])]);
    db.write(|tx| {
        tx.execute_batch(
            "DELETE FROM track_artists WHERE track_id = 1006;
             DELETE FROM tracks WHERE id = 1006;
             INSERT INTO library_changes (library_id, entity_type, entity_id, op, changed_at)
             VALUES (1, 'track', 1006, 'delete', 0);",
        )
    })
    .await
    .unwrap();
    // The search that notices the change answers from the index that still names the track.
    let (status, body) = json(&app, uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((&body["sections"], &body["top"]), (&json!([]), &Value::Null));
}

#[tokio::test]
async fn the_index_follows_the_library() {
    let (_temp, db, app) = app_with_catalog().await;
    let uri = "/api/v1/libraries/1/search?q=octopus";
    let (_, body) = json(&app, uri).await;
    assert_eq!(body["sections"], json!([]));
    db.write(|tx| {
        let title = "Octopus's Garden".into();
        Track { title, sort_key: b"o".to_vec(), ..Track::new(1007, 1, 101) }.insert(tx)?;
        tx.execute(
            "INSERT INTO library_changes (library_id, entity_type, entity_id, op, changed_at)
             VALUES (1, 'track', 1007, 'upsert', 0)",
            [],
        )
    })
    .await
    .unwrap();
    // The search that notices the change answers from the old index while a new one builds.
    let body = eventually("the index catches up", async || {
        let (_, body) = json(&app, uri).await;
        (body["sections"] != json!([])).then_some(body)
    })
    .await;
    assert_eq!(sections(&body), [("tracks".to_owned(), vec!["1007".to_owned()])]);
}

#[tokio::test]
async fn problems() {
    let (_temp, _db, app) = app_with_catalog().await;
    for (uri, status) in [
        ("/api/v1/libraries/9/search?q=a", StatusCode::NOT_FOUND),
        ("/api/v1/libraries/1/search", StatusCode::UNPROCESSABLE_ENTITY),
        ("/api/v1/libraries/1/search?q=", StatusCode::UNPROCESSABLE_ENTITY),
        ("/api/v1/libraries/1/search?q=a&sectionLimit=0", StatusCode::UNPROCESSABLE_ENTITY),
        ("/api/v1/libraries/1/search?q=a&sectionLimit=51", StatusCode::UNPROCESSABLE_ENTITY),
        ("/api/v1/libraries/1/search?q=a&types=songs", StatusCode::UNPROCESSABLE_ENTITY),
        // Filters and sections not built yet are refused rather than ignored.
        ("/api/v1/libraries/1/search?q=a&types=lyrics", StatusCode::UNPROCESSABLE_ENTITY),
        ("/api/v1/libraries/1/search?q=a&types=tracks,lyrics", StatusCode::UNPROCESSABLE_ENTITY),
        ("/api/v1/libraries/1/search?q=a&genre=rock", StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let (actual, content_type, _) = send(app.clone(), "GET", uri).await;
        assert_eq!(actual, status, "{uri}");
        assert_eq!(content_type.as_deref(), Some("application/problem+json"), "{uri}");
    }
}
