//! `Idempotency-Key` on a write: it applies once, and a repeat is answered as the first was.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use serde_json::{Value, json};

use super::Idempotency;
use crate::api::testing::{TOKEN, add_device, add_user, app_with_tracks, conn, respond};
use crate::db::testing::access;
use crate::db::{Database, retention};

const URI: &str = "/api/v1/libraries/1/playlists";
const DAY: i64 = 24 * 60 * 60 * 1000;

/// `method` on `uri` as `token`, with `body` and `key` if given: the status and the JSON body,
/// `Null` if there is none.
async fn send(
    app: &Router,
    token: &str,
    method: &str,
    uri: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    if let Some(key) = key {
        request = request.header("idempotency-key", key);
    }
    let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
    let (status, _, body) = respond(app.clone(), request.body(body).unwrap()).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The number of playlists there are that a test made, titled `A` or `B`.
async fn playlists(db: &Database) -> i64 {
    db.read(|conn| {
        conn.query_row("SELECT count(*) FROM playlists WHERE title IN ('A', 'B')", [], |row| {
            row.get(0)
        })
    })
    .await
    .unwrap()
}

/// A server with a second user, 2, who reaches library 1.
async fn server() -> (Arc<Database>, Router) {
    let (temp, db, app) = app_with_tracks().await;
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 3, "user2");
    access(&conn, 2, 1).unwrap();
    // The directory is kept until the process ends; the test is short.
    std::mem::forget(temp);
    (db, app)
}

/// A delete is answered `204` again, not `404` for what it deleted, when its key comes again.
#[tokio::test]
async fn a_delete_repeated_with_its_key_is_answered_as_the_first_was() {
    let (_db, app) = server().await;
    let (_, made) = send(&app, TOKEN, "POST", URI, None, Some(json!({ "title": "A" }))).await;
    let uri = format!("{URI}/{}", made["id"].as_str().unwrap());
    assert_eq!(send(&app, TOKEN, "DELETE", &uri, Some("d"), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(send(&app, TOKEN, "DELETE", &uri, Some("d"), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(send(&app, TOKEN, "DELETE", &uri, None, None).await.0, StatusCode::NOT_FOUND);
}

/// An answer is its account's own: another account sending the key gets nothing of it and is
/// asked afresh, and its own answer is stored beside the first.
#[tokio::test]
async fn a_key_is_the_callers_alone() {
    let (db, app) = server().await;
    let a = send(&app, TOKEN, "POST", URI, Some("k"), Some(json!({ "title": "A" }))).await;
    let b = send(&app, "user2", "POST", URI, Some("k"), Some(json!({ "title": "B" }))).await;
    assert_eq!((a.0, b.0), (StatusCode::CREATED, StatusCode::CREATED));
    assert_eq!((a.1["title"].clone(), b.1["title"].clone()), (json!("A"), json!("B")));
    assert_ne!(a.1["id"], b.1["id"]);
    assert_eq!(playlists(&db).await, 2);
    // Each repeat is its own.
    let again = send(&app, "user2", "POST", URI, Some("k"), Some(json!({ "title": "B" }))).await;
    assert_eq!(again, b);
    assert_eq!(send(&app, TOKEN, "POST", URI, Some("k"), Some(json!({ "title": "A" }))).await, a);
    assert_eq!(playlists(&db).await, 2);
}

/// An answer is remembered for 8 days, and forgotten once the sweep runs after that.
#[tokio::test]
async fn an_answer_is_kept_for_eight_days() {
    let (db, app) = server().await;
    let first = send(&app, TOKEN, "POST", URI, Some("k"), Some(json!({ "title": "A" }))).await;
    let age = |ms: i64| {
        let db = db.clone();
        async move {
            db.write(move |tx| {
                tx.execute(
                    "UPDATE idempotency_keys SET created_at = ?1",
                    [crate::db::now_ms() - ms],
                )
            })
            .await
            .unwrap();
            retention::sweep(&db, crate::db::now_ms()).await;
        }
    };
    age(7 * DAY).await;
    assert_eq!(
        send(&app, TOKEN, "POST", URI, Some("k"), Some(json!({ "title": "A" }))).await,
        first
    );
    assert_eq!(playlists(&db).await, 1);
    age(8 * DAY + 1000).await;
    let again = send(&app, TOKEN, "POST", URI, Some("k"), Some(json!({ "title": "A" }))).await;
    assert_eq!(again.0, StatusCode::CREATED);
    assert_ne!(again.1["id"], first.1["id"]);
    assert_eq!(playlists(&db).await, 2);
}

/// A refused write is not stored: the key asks afresh, and succeeds once the request is right.
#[tokio::test]
async fn a_refused_write_is_not_remembered() {
    let (db, app) = server().await;
    let refused = send(&app, TOKEN, "POST", URI, Some("k"), Some(json!({ "title": " " }))).await;
    assert_eq!(refused.0, StatusCode::UNPROCESSABLE_ENTITY);
    let made = send(&app, TOKEN, "POST", URI, Some("k"), Some(json!({ "title": "A" }))).await;
    assert_eq!(made.0, StatusCode::CREATED);
    assert_eq!(playlists(&db).await, 1);
}

/// Two requests with one key at once take turns: the second is the first's answer.
#[tokio::test]
async fn requests_with_one_key_at_once_apply_once() {
    let (db, app) = server().await;
    let body = || Some(json!({ "title": "A" }));
    let (a, b, c) = tokio::join!(
        send(&app, TOKEN, "POST", URI, Some("k"), body()),
        send(&app, TOKEN, "POST", URI, Some("k"), body()),
        send(&app, TOKEN, "POST", URI, Some("k"), body()),
    );
    assert_eq!(a.0, StatusCode::CREATED);
    assert_eq!((&b, &c), (&a, &a));
    assert_eq!(playlists(&db).await, 1);
}

/// A key's turn is forgotten once no request waits on it, so keys answered leave nothing behind.
#[tokio::test]
async fn a_keys_turn_is_forgotten_once_its_requests_are_answered() {
    let (db, _app) = server().await;
    let idempotency = Arc::new(Idempotency::default());
    let work = || async { StatusCode::NO_CONTENT.into_response() };
    let (a, b) = tokio::join!(
        idempotency.once(db.clone(), 1, "k".to_owned(), work()),
        idempotency.once(db.clone(), 1, "k".to_owned(), work()),
    );
    assert_eq!((a.status(), b.status()), (StatusCode::NO_CONTENT, StatusCode::NO_CONTENT));
    idempotency.once(db.clone(), 1, "other".to_owned(), work()).await;
    assert!(idempotency.gates.lock().unwrap().is_empty());
}

/// A key that is empty or too long is `422` on any write.
#[tokio::test]
async fn a_bad_key_is_refused_on_any_write() {
    let (_db, app) = server().await;
    for key in ["", &"k".repeat(129)] {
        let (status, _) = send(&app, TOKEN, "DELETE", &format!("{URI}/1"), Some(key), None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{key:?}");
    }
}
