use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, header};
use jewelcase_plugins::Permission::{LibraryRead, SearchActivity, Searched};
use serde_json::{Value, json};

use super::super::testing::{add_device, add_user, app_with_tracks, json, owner_request, respond};
use super::{KEPT, WINDOW_MS};
use crate::db::Database;
use crate::db::testing::{Artist, Track, access};
use crate::plugins::personal::set_search_sharing;
use crate::plugins::testing::{connect, enable, grant, install, manifest, plugins};

const URI: &str = "/api/v1/libraries/1/recent-searches";

/// Records `body` as the owner, and answers the status and the JSON body.
async fn record(app: &Router, body: Value) -> (StatusCode, Value) {
    let request = owner_request("POST", URI)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, _, body) = respond(app.clone(), request).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The owner's recent searches in library 1, newest first.
async fn queries(app: &Router) -> Vec<String> {
    let (status, list) = json(app, URI).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items = list["items"].as_array().unwrap();
    items.iter().map(|item| item["query"].as_str().unwrap().to_owned()).collect()
}

async fn send(app: &Router, method: &str, uri: &str) -> StatusCode {
    let request = owner_request(method, uri).body(Body::empty()).unwrap();
    respond(app.clone(), request).await.0
}

/// Moves every recent search `ms` into the past.
async fn age(db: &Database, ms: i64) {
    db.write(move |tx| {
        tx.execute("UPDATE recent_searches SET searched_at = searched_at - ?1", [ms])
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_search_is_recorded_with_what_was_selected_and_listed_newest_first() {
    let (_temp, _db, app) = app_with_tracks().await;
    let (status, first) = record(&app, json!({ "query": "  aurora  " })).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!((first["query"].as_str(), &first["selected"]), (Some("aurora"), &Value::Null));
    assert!(first["searchedAt"].as_str().unwrap().ends_with('Z'));

    let (status, second) =
        record(&app, json!({ "query": "signal", "selected": { "type": "album", "id": "101" } }))
            .await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    assert_eq!(second["selected"], json!({ "type": "album", "id": "101" }));
    assert_eq!(queries(&app).await, ["signal", "aurora"]);
}

#[tokio::test]
async fn the_same_words_again_move_to_the_top_under_a_new_id() {
    let (_temp, _db, app) = app_with_tracks().await;
    let (_, first) = record(&app, json!({ "query": "Björk" })).await;
    record(&app, json!({ "query": "signal" })).await;
    let (_, again) =
        record(&app, json!({ "query": "bjork", "selected": { "type": "track", "id": "1" } })).await;
    assert_eq!(queries(&app).await, ["bjork", "signal"], "ignoring case and accents");
    assert_ne!(first["id"], again["id"]);
}

#[tokio::test]
async fn only_the_newest_within_the_window_are_kept() {
    let (_temp, db, app) = app_with_tracks().await;
    for n in 0..KEPT + 3 {
        record(&app, json!({ "query": format!("search {n}") })).await;
    }
    let kept = queries(&app).await;
    assert_eq!(kept.len(), KEPT as usize);
    assert_eq!(kept[0], format!("search {}", KEPT + 2));

    age(&db, WINDOW_MS + 1).await;
    assert!(queries(&app).await.is_empty(), "aged out of the window");
    record(&app, json!({ "query": "fresh" })).await;
    let stored: i64 = db
        .read(|conn| conn.query_row("SELECT count(*) FROM recent_searches", [], |row| row.get(0)))
        .await
        .unwrap();
    assert_eq!(stored, 1, "what aged out is deleted, not hidden");
}

#[tokio::test]
async fn what_cannot_be_searched_or_selected_is_refused() {
    let (_temp, _db, app) = app_with_tracks().await;
    for body in [
        json!({ "query": "  ...  " }),
        json!({ "query": "x".repeat(501) }),
        json!({ "query": "a", "selected": { "type": "playlist", "id": "1" } }),
        json!({ "query": "a", "selected": { "type": "track", "id": "01" } }),
        // Track 3 is in library 2.
        json!({ "query": "a", "selected": { "type": "track", "id": "3" } }),
    ] {
        let (status, problem) = record(&app, body.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}: {problem}");
    }
    assert!(queries(&app).await.is_empty());
}

#[tokio::test]
async fn one_can_be_removed_or_all_cleared_and_never_anyone_elses() {
    let (temp, _db, app) = app_with_tracks().await;
    let (_, first) = record(&app, json!({ "query": "aurora" })).await;
    record(&app, json!({ "query": "signal" })).await;

    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    access(&conn, 2, 1).unwrap();
    let theirs = format!("{URI}/{}", first["id"].as_str().unwrap());
    let as_them = axum::http::Request::builder()
        .method("DELETE")
        .uri(&theirs)
        .header(header::AUTHORIZATION, "Bearer user-token")
        .body(Body::empty())
        .unwrap();
    assert_eq!(respond(app.clone(), as_them).await.0, StatusCode::NOT_FOUND);
    let as_them = axum::http::Request::builder()
        .uri(URI)
        .header(header::AUTHORIZATION, "Bearer user-token")
        .body(Body::empty())
        .unwrap();
    let (_, _, body) = respond(app.clone(), as_them).await;
    let listed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(listed["items"], json!([]), "each user sees only their own");

    assert_eq!(send(&app, "DELETE", &theirs).await, StatusCode::NO_CONTENT);
    assert_eq!(send(&app, "DELETE", &theirs).await, StatusCode::NOT_FOUND, "already gone");
    assert_eq!(queries(&app).await, ["signal"]);
    assert_eq!(send(&app, "DELETE", URI).await, StatusCode::NO_CONTENT);
    assert!(queries(&app).await.is_empty());
}

#[tokio::test]
async fn a_search_is_kept_per_library() {
    let (_temp, _db, app) = app_with_tracks().await;
    record(&app, json!({ "query": "aurora" })).await;
    let (status, other) = json(&app, "/api/v1/libraries/2/recent-searches").await;
    assert_eq!((status, &other["items"]), (StatusCode::OK, &json!([])));
}

/// Shares the owner's searches with `plugin`, which is allowed them, enabled in library 1, and
/// connected by the owner.
async fn share_with_a_plugin(temp: &tempfile::TempDir, db: &Arc<Database>) {
    let mut searcher = manifest("plugin", &[LibraryRead, SearchActivity, Searched]);
    searcher["personalSettings"] = json!({ "properties": { "token": { "type": "string" } } });
    install(db, searcher).await;
    let plugins = plugins(temp, db);
    grant(&plugins, "plugin", &[SearchActivity, Searched], &[LibraryRead]).await;
    enable(&plugins, "plugin", true).await;
    connect(db, "plugin", 1, json!({}), 0).await;
    let shared = db.write(|tx| set_search_sharing(tx, "plugin", 1, true, true)).await.unwrap();
    assert!(shared.is_ok());
}

/// What a search found: its tracks, albums, and artists.
type Found = (i64, i64, i64);

/// The events noted for the `searched` hook: each kind and search, and what a search found.
async fn events(db: &Database) -> Vec<(String, Option<i64>, Option<Found>)> {
    db.read(|conn| {
        conn.prepare(
            "SELECT kind, search_id, found_tracks, found_albums, found_artists FROM search_events \
             ORDER BY seq",
        )?
        .query_map([], |row| {
            let found: (Option<i64>, Option<i64>, Option<i64>) =
                (row.get(2)?, row.get(3)?, row.get(4)?);
            let found = match found {
                (Some(tracks), Some(albums), Some(artists)) => Some((tracks, albums, artists)),
                _ => None,
            };
            Ok((row.get(0)?, row.get(1)?, found))
        })?
        .collect()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn what_was_found_is_noted_only_for_a_user_who_shares() {
    let (temp, db, app) = app_with_tracks().await;
    connect(&db, "other", 1, json!({}), 0).await;
    // Another user shares with a plugin, which says nothing of the owner.
    db.write(|tx| {
        add_user(tx, 2, "user");
        Ok(())
    })
    .await
    .unwrap();
    connect(&db, "other", 2, json!({}), 0).await;
    db.write(|tx| {
        tx.execute("UPDATE plugin_connections SET searches_shared_at = 0 WHERE user_id = 2", [])
    })
    .await
    .unwrap();
    let (_, first) = record(&app, json!({ "query": "b" })).await;
    let (_, removed) = record(&app, json!({ "query": "c" })).await;
    let removed = format!("{URI}/{}", removed["id"].as_str().unwrap());
    assert_eq!(send(&app, "DELETE", &removed).await, StatusCode::NO_CONTENT);
    assert!(
        events(&db).await.is_empty(),
        "the owner is connected, but shares with no plugin, though another user does"
    );

    share_with_a_plugin(&temp, &db).await;
    db.write(|tx| Artist::new(301, 1, "A").insert(tx)).await.unwrap();
    let (_, second) = record(&app, json!({ "query": "a" })).await;
    let id = |search: &Value| search["id"].as_str().unwrap().parse::<i64>().unwrap();
    // Replaced by the same words: not a removal, so nothing to forget.
    let (_, again) = record(&app, json!({ "query": "A" })).await;
    assert_eq!(
        send(&app, "DELETE", &format!("{URI}/{}", id(&first))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(send(&app, "DELETE", URI).await, StatusCode::NO_CONTENT);
    assert_eq!(
        events(&db).await,
        [
            ("searched".into(), Some(id(&second)), Some((2, 0, 1))),
            ("searched".into(), Some(id(&again)), Some((2, 0, 1))),
            ("forgotten".into(), Some(id(&first)), None),
            ("forgotten".into(), Some(id(&again)), None),
        ],
        "both of library 1's tracks and its one artist are called A, and its album is untitled"
    );
}

#[tokio::test]
async fn only_a_noted_search_keeps_what_it_found_and_keeps_all_of_it() {
    let (_temp, db, _app) = app_with_tracks().await;
    let insert = |found: &'static str| {
        let db = db.clone();
        async move {
            db.write(move |tx| {
                tx.execute(
                    &format!(
                        "INSERT INTO search_events (user_id, library_id, kind, search_id, \
                                                    found_tracks, found_albums, found_artists, \
                                                    created_at) VALUES (1, 1, {found}, 0)"
                    ),
                    [],
                )
            })
            .await
        }
    };
    assert!(insert("'searched', 5, 1, 1, 1").await.is_ok(), "a search, with all three counts");
    assert!(insert("'searched', 5, 1, NULL, 1").await.is_err(), "a search missing a count");
    assert!(insert("'forgotten', 5, 0, NULL, NULL").await.is_err(), "a forget with a count");
}

/// Library 1's search index is built the first time anything searches it, and rebuilt only once
/// the library's change feed moves. Adding a track here leaves the feed where it was, so an
/// index built while the owner's search was recorded would never be rebuilt, and the search that
/// follows would miss the track.
#[tokio::test]
async fn a_search_is_not_counted_for_a_user_who_shares_nothing() {
    let (_temp, db, app) = app_with_tracks().await;
    let (status, recorded) = record(&app, json!({ "query": "a" })).await;
    assert_eq!(status, StatusCode::CREATED, "{recorded}");
    db.write(|tx| {
        Track { title: "A".into(), sort_key: b"a".to_vec(), ..Track::new(4, 1, 101) }.insert(tx)
    })
    .await
    .unwrap();
    let (status, found) = json(&app, "/api/v1/libraries/1/search?q=a&types=tracks").await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["sections"][0]["total"], 3, "this search built the index, not the one before");
}
