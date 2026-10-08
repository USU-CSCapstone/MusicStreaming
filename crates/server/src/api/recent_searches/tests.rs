use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, header};
use serde_json::{Value, json};

use super::super::testing::{add_device, add_user, app_with_tracks, json, owner_request, respond};
use super::{KEPT, WINDOW_MS};
use crate::db::Database;

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
    conn.execute("INSERT INTO library_access VALUES (2, 1, 0)", []).unwrap();
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

/// The events noted for the `searched` hook: each kind and search.
async fn events(db: &Database) -> Vec<(String, Option<i64>)> {
    db.read(|conn| {
        conn.prepare("SELECT kind, search_id FROM search_events ORDER BY seq")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn what_was_found_is_kept_and_noted_only_for_a_user_who_shares() {
    let (_temp, db, app) = app_with_tracks().await;
    let (_, first) = record(&app, json!({ "query": "a" })).await;
    let found: i64 = db
        .read(|conn| conn.query_row("SELECT found_tracks FROM recent_searches", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(found, 2, "both of library 1's tracks are called A");
    assert!(events(&db).await.is_empty(), "the owner shares with no plugin");

    db.write(|tx| tx.execute("INSERT INTO plugin_search_sharing VALUES ('p', 1, 0)", []))
        .await
        .unwrap();
    let (_, second) = record(&app, json!({ "query": "b" })).await;
    let id = |search: &Value| search["id"].as_str().unwrap().parse::<i64>().unwrap();
    // Replaced by the same words: not a removal, so nothing to forget.
    let (_, again) = record(&app, json!({ "query": "B" })).await;
    assert_eq!(
        send(&app, "DELETE", &format!("{URI}/{}", id(&first))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(send(&app, "DELETE", URI).await, StatusCode::NO_CONTENT);
    assert_eq!(
        events(&db).await,
        [
            ("searched".into(), Some(id(&second))),
            ("searched".into(), Some(id(&again))),
            ("forgotten".into(), Some(id(&first))),
            ("forgotten".into(), Some(id(&again))),
        ]
    );
}
