use std::io::Read;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use flate2::read::GzDecoder;
use serde_json::{Value, json};

use super::super::AppState;
use super::super::testing::socket::{Socket, connect_with, next, next_of, serve};
use super::super::testing::{
    TOKEN, add_device, add_user, app_with_tracks, call, conn, eventually, owner_request, respond,
    router_over, state,
};
use crate::db::testing::{Track, access};
use crate::queue::{Queues, list_hash};

const QUEUE: &str = "11111111-1111-4111-8111-111111111111";
const OTHER: &str = "22222222-2222-4222-8222-222222222222";
const KEY: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const KEY2: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

/// The API over library 1 (tracks 1 and 2) and library 2 (track 3), whose state a test keeps:
/// the owner on device 1 as `TOKEN`, and user 2, who reaches only library 1, on device 3 as
/// `user2`.
async fn setup() -> (tempfile::TempDir, AppState, Router) {
    let (temp, db, _app) = app_with_tracks().await;
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 3, "user2");
    access(&conn, 2, 1).unwrap();
    let state = state(&temp, db);
    let app = router_over(state.clone());
    (temp, state, app)
}

/// A start of `queue` over `tracks` of library 1, keyed `key`, playing `start` first.
fn start_body(queue: &str, key: &str, tracks: &[&str], start: u64) -> Value {
    json!({
        "queueId": queue, "context": { "type": "album", "id": "101" },
        "source": { "key": key, "libraryId": "1", "tracks": tracks }, "start": start,
    })
}

/// `startQueue` as `token`.
async fn start(app: &Router, token: &str, body: Value) -> (StatusCode, Value) {
    call(app, token, "POST", "/api/v1/me/queue", Some(body)).await
}

/// `getQueue` as the owner with `headers` added: the status, headers and body.
async fn get(app: &Router, headers: &[(&str, &str)]) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut request = owner_request("GET", "/api/v1/me/queue");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    respond(app.clone(), request.body(Body::empty()).unwrap()).await
}

/// `getQueue` as the owner, as JSON.
async fn snapshot(app: &Router) -> Value {
    let (status, _, body) = get(app, &[]).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).unwrap()
}

/// The `ETag` of `headers`.
fn etag(headers: &HeaderMap) -> String {
    headers.get(header::ETAG).unwrap().to_str().unwrap().to_owned()
}

/// The next message of the connection that is not the state every connection is sent.
async fn message(socket: &mut Socket) -> Value {
    loop {
        let message = next(socket).await;
        if message["type"] != "devices" && message["type"] != "player" {
            return message;
        }
    }
}

/// Every object key in `value`, at any depth.
fn keys(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                found.push(key.clone());
                keys(value, found);
            }
        }
        Value::Array(values) => values.iter().for_each(|value| keys(value, found)),
        _ => {}
    }
}

#[tokio::test]
async fn snapshot_has_no_tracks() {
    let (_temp, _state, app) = setup().await;
    let (status, body) = call(&app, TOKEN, "GET", "/api/v1/me/queue", None).await;
    assert_eq!((status, &body["code"]), (StatusCode::NOT_FOUND, &json!("not_found")));

    let (status, answer) = start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2", "1"], 1)).await;
    let list = list_hash(&[1, 2, 1]);
    assert_eq!((status, &answer), (StatusCode::OK, &json!({ "list": list, "length": 3 })));

    let snapshot = snapshot(&app).await;
    assert_eq!(
        snapshot,
        json!({
            "queueId": QUEUE, "version": 0, "context": { "type": "album", "id": "101" },
            "state": {
                "sources": [{ "key": KEY, "libraryId": "1", "list": list, "length": 3 }],
                "copies": [], "manual": [],
                "order": [{ "view": { "key": KEY }, "start": 0, "len": 3 }],
                "cursor": 2, "current": format!("{KEY}.1"), "history": [], "nextPlay": 0,
                "shuffle": null, "repeat": "off", "adjusted": false,
            },
        })
    );
    let mut found = Vec::new();
    keys(&snapshot, &mut found);
    assert!(!found.iter().any(|key| key.contains("track")), "{found:?}");
}

#[tokio::test]
async fn snapshot_is_conditional() {
    let (temp, state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;

    let (status, headers, body) = get(&app, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let tag = etag(&headers);
    assert!(tag.starts_with('"') && tag.contains(QUEUE) && !body.is_empty(), "{tag}");

    for known in [tag.clone(), format!("\"other\", {tag}"), format!("W/{tag}"), "*".to_owned()] {
        let (status, headers, body) = get(&app, &[("if-none-match", &known)]).await;
        assert_eq!(
            (status, etag(&headers), body.len()),
            (StatusCode::NOT_MODIFIED, tag.clone(), 0)
        );
    }
    let (status, _, _) = get(&app, &[("if-none-match", "\"other\"")]).await;
    assert_eq!(status, StatusCode::OK);

    // Another queue is another tag.
    start(&app, TOKEN, start_body(OTHER, KEY2, &["2"], 0)).await;
    let (status, headers, _) = get(&app, &[("if-none-match", &tag)]).await;
    assert_eq!(status, StatusCode::OK);
    let second = etag(&headers);
    assert_ne!(second, tag);

    // After a restart the queue is the same and no tag from before matches.
    let db = state.db.clone();
    state.queues.save(&db).await;
    let restarted = AppState { queues: Arc::new(Queues::load(&db).await.unwrap()), ..state };
    let app = router_over(restarted);
    let (status, headers, _) = get(&app, &[("if-none-match", &second)]).await;
    assert_eq!(status, StatusCode::OK, "the tag from before the restart");
    assert_ne!(etag(&headers), second);
    assert_eq!(snapshot(&app).await["queueId"], json!(OTHER));
    let (status, ..) = get(&app, &[("if-none-match", &etag(&headers))]).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    drop(temp);
}

#[tokio::test]
async fn snapshot_is_gzipped() {
    let (_temp, _state, app) = setup().await;
    let tracks: Vec<String> = (0..200).map(|_| "1".to_owned()).collect();
    let tracks: Vec<&str> = tracks.iter().map(String::as_str).collect();
    start(&app, TOKEN, start_body(QUEUE, KEY, &tracks, 0)).await;

    let (status, headers, plain) = get(&app, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(header::CONTENT_ENCODING), None);

    let (status, headers, zipped) = get(&app, &[("accept-encoding", "gzip")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_ENCODING], "gzip");
    assert_eq!(headers[header::CONTENT_TYPE], "application/json");
    let mut decoded = Vec::new();
    GzDecoder::new(zipped.as_slice()).read_to_end(&mut decoded).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&decoded).unwrap(),
        serde_json::from_slice::<Value>(&plain).unwrap()
    );

    // Only the snapshot is gzipped, not a start's answer.
    let request = owner_request("POST", "/api/v1/me/queue")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT_ENCODING, "gzip")
        .body(Body::from(start_body(OTHER, KEY2, &["1"], 0).to_string()))
        .unwrap();
    let (status, headers, body) = respond(app.clone(), request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(header::CONTENT_ENCODING), None);
    assert!(serde_json::from_slice::<Value>(&body).is_ok());
}

#[tokio::test]
async fn start_refuses_unreachable_tracks() {
    let (_temp, _state, app) = setup().await;
    let in_library = |library: &str, tracks: &[&str]| {
        let mut body = start_body(QUEUE, KEY, tracks, 0);
        body["source"]["libraryId"] = json!(library);
        body
    };
    // Track 3 is in library 2, which user 2 does not reach; track 99 and library 9 are not
    // there at all. The refusals say nothing of which.
    let (status, unreachable) = start(&app, "user2", in_library("2", &["3"])).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unreachable}");
    let (_, missing) = start(&app, "user2", in_library("2", &["99"])).await;
    let (_, missing_library) = start(&app, "user2", in_library("9", &["99"])).await;
    assert_eq!((&unreachable, &unreachable), (&missing, &missing_library));
    for refused in [
        in_library("1", &["3"]),
        in_library("1", &["1", "99"]),
        in_library("1", &["01"]),
        in_library("1", &["-1"]),
    ] {
        let (status, _) = start(&app, "user2", refused.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    }
    let (status, _) = call(&app, "user2", "GET", "/api/v1/me/queue", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "nothing started");

    // The owner reaches every library, and a start there is accepted.
    let (status, _) = start(&app, TOKEN, in_library("2", &["3"])).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn start_refuses_malformed_requests() {
    let (_temp, _state, app) = setup().await;
    let good = start_body(QUEUE, KEY, &["1", "2"], 0);
    let with = |path: &[&str], value: Value| {
        let mut body = good.clone();
        let mut at = &mut body;
        for name in path {
            at = &mut at[*name];
        }
        *at = value;
        body
    };
    let mut recipe = good.clone();
    recipe["source"] = json!({ "key": KEY, "libraryId": "1", "recipe": { "type": "tracks" } });
    for (what, body) in [
        ("a queue ID that is not a UUID", with(&["queueId"], json!("queue"))),
        ("a key that is not a UUID", with(&["source", "key"], json!("key"))),
        ("a start past the end", with(&["start"], json!(2))),
        ("no tracks", with(&["source", "tracks"], json!([]))),
        ("a context that is not an object", with(&["context"], json!("album"))),
        ("a context with no type", with(&["context"], json!({ "id": "1" }))),
        ("a recipe, which is not built yet", recipe),
    ] {
        let (status, body) = start(&app, TOKEN, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{what}: {body}");
    }
    let (status, _) = call(&app, TOKEN, "GET", "/api/v1/me/queue", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "nothing started");
}

#[tokio::test]
async fn retried_start_is_noop() {
    let (_temp, _state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, TOKEN).await;

    let first = start_body(QUEUE, KEY, &["1", "2"], 0);
    let (status, answer) = start(&app, TOKEN, first.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let (_, headers, _) = get(&app, &[]).await;

    // The same request again, and the same queue ID with other tracks, answer as the start did
    // for the tracks they send, and change nothing.
    assert_eq!(start(&app, TOKEN, first).await, (StatusCode::OK, answer.clone()));
    let (status, other) = start(&app, TOKEN, start_body(QUEUE, KEY2, &["2"], 0)).await;
    assert_eq!(
        (status, &other),
        (StatusCode::OK, &json!({ "list": list_hash(&[2]), "length": 1 }))
    );
    let (status, after, _) = get(&app, &[("if-none-match", &etag(&headers))]).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert_eq!(etag(&after), etag(&headers));
    assert_eq!(snapshot(&app).await["state"]["sources"][0]["key"], json!(KEY));

    // A queue that replaced it is not replaced back by the retry, and it is the next thing the
    // account's devices hear after the first start: the retries sent nothing.
    start(&app, TOKEN, start_body(OTHER, KEY2, &["2"], 0)).await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    assert_eq!(snapshot(&app).await["queueId"], json!(OTHER));
    assert_eq!(message(&mut socket).await, json!({ "type": "queueReplaced", "queueId": QUEUE }));
    assert_eq!(message(&mut socket).await, json!({ "type": "queueReplaced", "queueId": OTHER }));
    socket_is_quiet(&mut socket).await;
}

/// Asserts that no `queueReplaced` is waiting on `socket`: a ping sent now is answered with
/// none before it.
async fn socket_is_quiet(socket: &mut Socket) {
    use futures_util::SinkExt;
    socket.send(tokio_tungstenite::tungstenite::Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    loop {
        let message = next(socket).await;
        assert_ne!(message["type"], "queueReplaced", "{message}");
        if message["type"] == "pong" {
            return;
        }
    }
}

#[tokio::test]
async fn queue_is_per_account() {
    let (_temp, _state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut stranger = connect_with(address, "user2").await;
    next_of(&mut stranger, "player").await;

    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    let (status, body) = call(&app, "user2", "GET", "/api/v1/me/queue", None).await;
    assert_eq!((status, &body["code"]), (StatusCode::NOT_FOUND, &json!("not_found")));
    socket_is_quiet(&mut stranger).await;

    // The same queue ID is another account's own, not the owner's queue to find.
    let (status, answer) = start(&app, "user2", start_body(QUEUE, KEY, &["2"], 0)).await;
    assert_eq!((status, &answer["length"]), (StatusCode::OK, &json!(1)));
    assert_eq!(message(&mut stranger).await["type"], "queueReplaced");
    let theirs = call(&app, "user2", "GET", "/api/v1/me/queue", None).await.1;
    assert_eq!(theirs["state"]["sources"][0]["length"], json!(1));
    assert_eq!(snapshot(&app).await["state"]["sources"][0]["length"], json!(2));
}

#[tokio::test]
async fn the_queue_is_saved_every_second_and_by_a_final_save() {
    let (temp, state, app) = setup().await;
    let db = state.db.clone();
    let rows = |temp: &tempfile::TempDir| -> Vec<(String, i64)> {
        let conn = conn(temp);
        let mut select =
            conn.prepare("SELECT queue_id, active FROM queues WHERE user_id = 1").unwrap();
        select
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };

    // The timer saves it, with no one asking.
    let saving = state.queues.keep_saved(db.clone());
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    let saved = eventually("the timer saves the queue", async || {
        Some(rows(&temp)).filter(|rows| !rows.is_empty())
    })
    .await;
    assert_eq!(saved, [(QUEUE.to_owned(), 1)]);
    saving.abort();

    // What was started after the timer stopped is saved by the save the server ends with.
    start(&app, TOKEN, start_body(OTHER, KEY2, &["2"], 0)).await;
    assert_eq!(rows(&temp), [(QUEUE.to_owned(), 1)], "nothing saves it meanwhile");
    state.queues.save(&db).await;
    assert_eq!(rows(&temp), [(OTHER.to_owned(), 1)], "an untouched queue is not kept");
    let restarted = AppState { queues: Arc::new(Queues::load(&db).await.unwrap()), ..state };
    assert_eq!(snapshot(&router_over(restarted)).await["queueId"], json!(OTHER));
}

#[tokio::test]
async fn a_start_may_be_far_larger_than_axums_default_body() {
    let (_temp, state, app) = setup().await;
    // A track with a 19-digit ID, as real ones have, so each place costs 22 bytes of JSON.
    let big = 4_000_000_000_000_000_000_i64;
    let db = state.db.clone();
    db.write(move |tx| {
        Track { title: "A".into(), sort_key: b"a".to_vec(), ..Track::new(big, 1, 101) }.insert(tx)
    })
    .await
    .unwrap();
    let id = big.to_string();
    let tracks = vec![id.as_str(); 200_000];
    let body = start_body(QUEUE, KEY, &tracks, 0);
    assert!(body.to_string().len() > 2 << 20, "past axum's default limit");
    let (status, answer) = start(&app, TOKEN, body).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer,
        json!({ "list": list_hash(&vec![big.cast_unsigned(); 200_000]), "length": 200_000 })
    );
}

#[tokio::test]
async fn a_start_past_the_body_limit_is_refused() {
    let (_temp, _state, app) = setup().await;
    let head = format!(
        r#"{{"queueId":"{QUEUE}","context":{{"type":"album"}},"source":{{"key":"{KEY}","libraryId":"1","tracks":["#
    );
    let mut body = head;
    body.push_str(&"\"1\",".repeat(super::BODY_LIMIT / 4 + 1));
    body.push_str(r#""1"]},"start":0}"#);
    let request = owner_request("POST", "/api/v1/me/queue")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap();
    let (status, _, answer) = respond(app.clone(), request).await;
    // The API's JSON extractor answers every body it cannot read as `validation_failed`.
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{}", String::from_utf8_lossy(&answer));
    let (status, _) = call(&app, TOKEN, "GET", "/api/v1/me/queue", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "nothing started");
}
