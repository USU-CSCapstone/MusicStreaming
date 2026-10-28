use axum::Router;
use axum::http::StatusCode;
use jewelcase_core::{Anchor, Edit, Source};
use serde_json::{Value, json};
use std::sync::Arc;

use super::{Queues, Uuid};
use crate::api::testing::{TOKEN, add_device, add_user, app_with_tracks, call, conn};
use crate::db::testing::access;

mod saved;

const QUEUE: &str = "5f0c6a52-3b7e-4d43-9a8e-0d1c2b3a4f5e";
const OTHER: &str = "6a1d0c52-3b7e-4d43-9a8e-0d1c2b3a4f5e";
const KEY: &str = "0b1b1b1b-3b7e-4d43-9a8e-0d1c2b3a4f5e";

/// [`app_with_tracks`], with user 2, who reaches only library 1, on device 2 as `user2`, and
/// user 3, who reaches it too, on device 3 as `user3`.
async fn setup() -> (tempfile::TempDir, Router) {
    let (temp, _db, app) = app_with_tracks().await;
    let conn = conn(&temp);
    for user in [2, 3] {
        add_user(&conn, user, "user");
        add_device(&conn, user, user, &format!("user{user}"));
        access(&conn, user, 1).unwrap();
    }
    (temp, app)
}

/// A request to start a queue `queue` over `tracks` of `library`, from the first track.
fn start(queue: &str, library: &str, tracks: &[&str]) -> Value {
    json!({
        "queueId": queue, "context": { "type": "album", "id": "101" }, "start": 0,
        "source": { "key": KEY, "libraryId": library, "tracks": tracks },
    })
}

/// Starts `body` as `token`, and returns the status.
async fn post(app: &Router, token: &str, body: Value) -> StatusCode {
    call(app, token, "POST", "/api/v1/me/queue", Some(body)).await.0
}

/// `getQueue` as `token`.
async fn get(app: &Router, token: &str) -> (StatusCode, Value) {
    call(app, token, "GET", "/api/v1/me/queue", None).await
}

#[tokio::test]
async fn before_a_start_there_is_no_queue() {
    let (_temp, app) = setup().await;
    assert_eq!(get(&app, TOKEN).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_start_makes_the_queue_the_snapshot_shows() {
    let (_temp, app) = setup().await;
    let mut body = start(QUEUE, "1", &["1", "2", "1"]);
    body["start"] = json!(1);
    assert_eq!(post(&app, TOKEN, body).await, StatusCode::NO_CONTENT);

    let (status, snapshot) = get(&app, TOKEN).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(snapshot["queueId"], QUEUE);
    assert_eq!(snapshot["version"], 0);
    assert_eq!(snapshot["context"], json!({ "type": "album", "id": "101" }));
    let state = &snapshot["state"];
    assert_eq!(
        state["sources"],
        json!([{ "key": KEY, "libraryId": "1", "tracks": ["1", "2", "1"] }])
    );
    assert_eq!(state["current"], json!(1));
    assert_eq!(state["order"], json!([0, 1, 2]));
    assert_eq!(state["cursor"], json!(2));
    assert_eq!(state["shuffle"], Value::Null);
}

#[tokio::test]
async fn a_start_can_shuffle() {
    let (_temp, app) = setup().await;
    let mut body = start(QUEUE, "1", &["1", "2", "1", "2"]);
    body["shuffle"] = json!({ "seed": 7 });
    assert_eq!(post(&app, TOKEN, body).await, StatusCode::NO_CONTENT);
    assert_eq!(get(&app, TOKEN).await.1["state"]["shuffle"]["seed"], 7);
}

#[tokio::test]
async fn a_new_start_replaces_the_queue() {
    let (_temp, app) = setup().await;
    post(&app, TOKEN, start(QUEUE, "1", &["1", "2"])).await;
    let mut second = start(OTHER, "2", &["3"]);
    second["context"] = json!({ "type": "library" });
    assert_eq!(post(&app, TOKEN, second).await, StatusCode::NO_CONTENT);
    let snapshot = get(&app, TOKEN).await.1;
    assert_eq!(snapshot["queueId"], OTHER);
    assert_eq!(snapshot["context"], json!({ "type": "library" }));
    assert_eq!(snapshot["state"]["sources"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_start_sent_again_changes_nothing() {
    let (_temp, app) = setup().await;
    let body = start(QUEUE, "1", &["1", "2"]);
    post(&app, TOKEN, body.clone()).await;
    let (_, played) = get(&app, TOKEN).await;
    assert_eq!(post(&app, TOKEN, body).await, StatusCode::NO_CONTENT);
    assert_eq!(get(&app, TOKEN).await.1, played);
}

/// The owner's queue holds `manual` tracks queued by hand after a start from `first`, and then
/// a new start from `second` carries them over.
#[test]
fn a_start_carries_the_manual_queue_over() {
    let queues = Queues::default();
    let source = |key: &str, tracks: &[&str]| Source {
        key: key.into(),
        library_id: "1".into(),
        tracks: tracks.iter().map(|track| (*track).into()).collect(),
    };
    let id = |text: &str| Uuid::try_from(text.to_owned()).unwrap();
    queues
        .replace(1, id(QUEUE), json!({ "type": "album" }), source("a", &["1", "2"]), 0, None)
        .unwrap();
    {
        let mut accounts = queues.lock();
        let queue = &mut accounts.get_mut(&1).unwrap().queue;
        let queued = Edit::Add { source: source("m", &["9", "8"]), before: Anchor::ManualEnd };
        queue.apply(&queued).unwrap();
    }
    queues.replace(1, id(OTHER), json!({ "type": "album" }), source("b", &["3"]), 0, None).unwrap();

    let accounts = queues.lock();
    let account = &accounts[&1];
    assert_eq!(account.version, 0);
    let tracks: Vec<_> = account
        .queue
        .manual()
        .map(|item| account.queue.track(&item).unwrap().track_id.to_owned())
        .collect();
    assert_eq!(tracks, ["9", "8"]);
    assert_eq!(account.queue.current().as_deref(), Some("b.0"));
}

/// A start sent again, once the queue has moved on, changes nothing of it.
#[test]
fn a_start_sent_again_keeps_what_happened_since() {
    let queues = Queues::default();
    let source =
        Source { key: "a".into(), library_id: "1".into(), tracks: vec!["1".into(), "2".into()] };
    let id = Uuid::try_from(QUEUE.to_owned()).unwrap();
    let context = json!({ "type": "album" });
    queues.replace(1, id.clone(), context.clone(), source.clone(), 0, None).unwrap();
    {
        let mut accounts = queues.lock();
        let account = accounts.get_mut(&1).unwrap();
        account.queue.apply(&Edit::Next).unwrap();
        account.version = 4;
    }
    let moved = queues.lock()[&1].queue.clone();
    queues.replace(1, id, context, source, 0, None).unwrap();
    assert_eq!(queues.lock()[&1].queue, moved);
    assert_eq!(queues.lock()[&1].version, 4);
}

#[tokio::test]
async fn a_failed_start_keeps_the_queue() {
    let (_temp, app) = setup().await;
    post(&app, TOKEN, start(QUEUE, "1", &["1", "2"])).await;
    let mut past_the_end = start(OTHER, "1", &["1"]);
    past_the_end["start"] = json!(1);
    assert_eq!(post(&app, TOKEN, past_the_end).await, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(get(&app, TOKEN).await.1["queueId"], QUEUE);
}

#[tokio::test]
async fn tracks_out_of_reach_are_refused_as_unknown_ones_are() {
    let (_temp, app) = setup().await;
    let refused = |body: Value| {
        let app = app.clone();
        async move {
            let (status, problem) =
                call(&app, "user2", "POST", "/api/v1/me/queue", Some(body)).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            problem
        }
    };
    // Track 3 is in library 2, which user 2 does not reach; 99 is nowhere.
    let out_of_reach = refused(start(QUEUE, "2", &["3"])).await;
    assert_eq!(out_of_reach, refused(start(QUEUE, "1", &["99"])).await, "an unknown track");
    assert_eq!(out_of_reach, refused(start(QUEUE, "1", &["1", "3"])).await, "one of several");
    assert_eq!(out_of_reach, refused(start(QUEUE, "9", &["3"])).await, "an unknown library");
    assert_eq!(out_of_reach, refused(start(QUEUE, "x", &["1"])).await, "a malformed library");
    assert_eq!(out_of_reach, refused(start(QUEUE, "1", &["01"])).await, "a malformed track");
    assert_eq!(get(&app, "user2").await.0, StatusCode::NOT_FOUND, "and nothing started");

    // Whoever reaches the library may start from its tracks, and the owner reaches both.
    assert_eq!(post(&app, "user2", start(QUEUE, "1", &["1"])).await, StatusCode::NO_CONTENT);
    assert_eq!(post(&app, TOKEN, start(OTHER, "2", &["3"])).await, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn an_access_taken_away_stops_new_starts() {
    let (temp, app) = setup().await;
    conn(&temp).execute("DELETE FROM library_access WHERE user_id = 2", []).unwrap();
    let status = post(&app, "user2", start(QUEUE, "1", &["1"])).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn missing_tracks_are_allowed() {
    let (temp, app) = setup().await;
    conn(&temp).execute("UPDATE tracks SET missing_since = 1 WHERE id = 2", []).unwrap();
    let status = post(&app, TOKEN, start(QUEUE, "1", &["1", "2"])).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_start_that_is_malformed_is_refused() {
    let (_temp, app) = setup().await;
    let big = "x".repeat(5000);
    let mut bad = vec![];
    for (field, value) in [
        ("context", json!({ "id": "101" })),
        ("context", json!({ "type": "album", "id": big })),
        ("context", json!("album")),
        ("queueId", json!("not-a-uuid")),
        ("start", json!(5)),
        ("start", json!(-1)),
    ] {
        let mut body = start(QUEUE, "1", &["1", "2"]);
        body[field] = value;
        bad.push(body);
    }
    let mut keyed = start(QUEUE, "1", &["1"]);
    keyed["source"]["key"] = json!("~0");
    bad.push(keyed);
    bad.push(start(QUEUE, "1", &[]));
    for body in bad {
        let status = post(&app, TOKEN, body.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
}

#[tokio::test]
async fn each_account_has_its_own_queue() {
    let (_temp, app) = setup().await;
    // Two accounts may even choose the same queue ID.
    post(&app, "user2", start(QUEUE, "1", &["1"])).await;
    post(&app, "user3", start(QUEUE, "1", &["2"])).await;
    assert_eq!(get(&app, TOKEN).await.0, StatusCode::NOT_FOUND);
    let tracks = |snapshot: Value| snapshot["state"]["sources"][0]["tracks"].clone();
    assert_eq!(tracks(get(&app, "user2").await.1), json!(["1"]));
    assert_eq!(tracks(get(&app, "user3").await.1), json!(["2"]));

    // One account starting another queue leaves the other's alone.
    post(&app, "user2", start(OTHER, "1", &["2", "1"])).await;
    assert_eq!(tracks(get(&app, "user3").await.1), json!(["2"]));
    assert_eq!(get(&app, "user3").await.1["queueId"], QUEUE);
}

/// `getQueue` as the owner with `If-None-Match: tag`, if any: the status, `ETag` and body length.
async fn conditional_get(app: &Router, tag: Option<&str>) -> (StatusCode, Option<String>, usize) {
    let mut request = axum::http::Request::builder()
        .uri("/api/v1/me/queue")
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    if let Some(tag) = tag {
        request = request.header(axum::http::header::IF_NONE_MATCH, tag);
    }
    let (status, headers, body) =
        crate::api::testing::respond(app.clone(), request.body(axum::body::Body::empty()).unwrap())
            .await;
    let etag = headers.get(axum::http::header::ETAG).map(|tag| tag.to_str().unwrap().to_owned());
    (status, etag, body.len())
}

#[tokio::test]
async fn the_snapshot_is_conditional_on_its_queue_and_version() {
    let (_temp, app) = setup().await;
    post(&app, TOKEN, start(QUEUE, "1", &["1", "2"])).await;

    let (status, etag, length) = conditional_get(&app, None).await;
    let etag = etag.unwrap();
    assert_eq!((status, etag.as_str()), (StatusCode::OK, format!("\"{QUEUE}.0\"").as_str()));
    assert!(length > 0);

    let (status, same, length) = conditional_get(&app, Some(&etag)).await;
    assert_eq!((status, same.as_ref(), length), (StatusCode::NOT_MODIFIED, Some(&etag), 0));
    let listed = format!("\"x\", {etag}");
    assert_eq!(conditional_get(&app, Some(&listed)).await.0, StatusCode::NOT_MODIFIED);

    for other in [format!("\"{QUEUE}.1\""), format!("\"{OTHER}.0\""), "*".to_owned()] {
        let (status, _, length) = conditional_get(&app, Some(&other)).await;
        assert_eq!(status, StatusCode::OK, "{other}");
        assert!(length > 0);
    }
}

#[tokio::test]
async fn a_new_start_keeps_the_accounts_repeat_mode_and_manual_queue() {
    // No endpoint edits a queue yet, so the edits are made as the server's own code would.
    let queues = Arc::new(Queues::default());
    let id = Uuid::try_from(QUEUE.to_owned()).unwrap();
    let source = |key: &str| Source {
        key: key.into(),
        library_id: "1".into(),
        tracks: vec!["1".into(), "2".into()],
    };
    queues.replace(1, id, json!({ "type": "album" }), source(KEY), 0, None).unwrap();
    {
        let mut accounts = queues.lock();
        let queue = &mut accounts.get_mut(&1).unwrap().queue;
        queue.apply(&Edit::Repeat { mode: jewelcase_core::Repeat::All }).unwrap();
        queue.apply(&Edit::Add { source: source("m"), before: Anchor::ManualEnd }).unwrap();
    }
    let next = Uuid::try_from(OTHER.to_owned()).unwrap();
    queues.replace(1, next, json!({ "type": "album" }), source(KEY), 0, Some(5)).unwrap();
    let accounts = queues.lock();
    let queue = &accounts[&1].queue;
    assert_eq!(queue.repeat(), jewelcase_core::Repeat::All);
    assert_eq!(queue.manual().count(), 2);
    assert!(queue.shuffled());
}
