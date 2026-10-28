//! Editing the queue: `editQueue`, and what it sends the account's connections.

use axum::Router;
use axum::http::StatusCode;
use futures_util::SinkExt;
use jewelcase_core::{Edit, Source};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::{KEY, OTHER, QUEUE, get, post, setup, start};
use crate::api::Uuid;
use crate::api::queue::Queues;
use crate::api::realtime::Hub;
use crate::api::testing::socket::{Socket, connect_with, next, next_of, serve};
use crate::api::testing::{TOKEN, call};

/// The key of a source an edit adds.
const ADDED: &str = "0b1b1b1b-3b7e-4d43-9a8e-0d1c2b3a4f6f";

/// An edit ID, one for each `n`.
fn edit_id(n: usize) -> String {
    format!("7e0d1c52-3b7e-4d43-9a8e-0d1c2b3a{n:04x}")
}

/// Sends `edit` as edit `n` for `queue`, as `token`.
async fn send(
    app: &Router,
    token: &str,
    queue: &str,
    n: usize,
    edit: Value,
) -> (StatusCode, Value) {
    let body = json!({ "editId": edit_id(n), "queueId": queue, "edit": edit });
    call(app, token, "POST", "/api/v1/me/queue/edits", Some(body)).await
}

/// An add of `tracks` of library 1 under `key`, at `before`.
fn add(key: &str, tracks: &[&str], before: Value) -> Value {
    json!({
        "type": "add", "before": before,
        "source": { "key": key, "libraryId": "1", "tracks": tracks },
    })
}

/// Whether the connection hears nothing of the queue before it answers a ping, which it does
/// after anything the server sent first.
async fn hears_no_queue(socket: &mut Socket) -> bool {
    socket.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    loop {
        match next(socket).await["type"].as_str().unwrap() {
            "pong" => return true,
            "queueEdit" | "queueReplaced" => return false,
            _ => {}
        }
    }
}

#[tokio::test]
async fn every_kind_of_edit_applies_bumps_the_version_and_reaches_the_accounts_connections() {
    let (_temp, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut here = connect_with(address, TOKEN).await;
    let mut there = connect_with(address, TOKEN).await;
    let mut other_account = connect_with(address, "user3").await;
    post(&app, TOKEN, start(QUEUE, "1", &["1", "2", "1", "2"])).await;

    let manual = json!({ "type": "manualEnd" });
    let edits = [
        add(ADDED, &["2"], manual),
        json!({ "type": "move", "items": [format!("{KEY}.3")], "before": { "type": "manualStart" } }),
        json!({ "type": "remove", "items": [format!("{KEY}.2")] }),
        json!({ "type": "jump", "item": format!("{KEY}.1") }),
        json!({ "type": "next" }),
        json!({ "type": "previous" }),
        json!({ "type": "shuffle", "on": true, "seed": 5 }),
        json!({ "type": "repeat", "mode": "all" }),
    ];
    for (n, edit) in edits.into_iter().enumerate() {
        let (status, answer) = send(&app, TOKEN, QUEUE, n, edit.clone()).await;
        assert_eq!((status, &answer), (StatusCode::OK, &json!({ "version": n + 1 })), "{edit}");
        for socket in [&mut here, &mut there] {
            let message = next_of(socket, "queueEdit").await;
            let sent = json!({
                "type": "queueEdit", "queueId": QUEUE, "version": n + 1,
                "editId": edit_id(n), "edit": edit,
            });
            assert_eq!(message, sent);
        }
    }
    assert_eq!(get(&app, TOKEN).await.1["version"], 8);
    assert!(hears_no_queue(&mut other_account).await, "another account hears nothing");
}

#[tokio::test]
async fn an_edit_sent_again_is_applied_once_and_answered_the_same() {
    let (_temp, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, TOKEN).await;
    post(&app, TOKEN, start(QUEUE, "1", &["1", "2"])).await;

    // Applied again, the add would be refused for its source's key.
    let first = add(ADDED, &["2"], json!({ "type": "manualEnd" }));
    let answered = send(&app, TOKEN, QUEUE, 1, first.clone()).await;
    assert_eq!(answered, (StatusCode::OK, json!({ "version": 1 })));
    send(&app, TOKEN, QUEUE, 2, json!({ "type": "next" })).await;
    assert_eq!(send(&app, TOKEN, QUEUE, 1, first).await, answered);

    let snapshot = get(&app, TOKEN).await.1;
    assert_eq!(snapshot["version"], 2);
    assert_eq!(snapshot["state"]["sources"].as_array().unwrap().len(), 2);
    next_of(&mut socket, "queueEdit").await;
    assert_eq!(next_of(&mut socket, "queueEdit").await["version"], 2);
    assert!(hears_no_queue(&mut socket).await, "the repeat was not sent");
}

#[test]
fn only_the_latest_edits_are_remembered() {
    let queues = Queues::default();
    let hub = Hub::default();
    let id = |text: &str| Uuid::try_from(text.to_owned()).unwrap();
    let source = Source { key: "a".into(), library_id: "1".into(), tracks: vec!["1".into()] };
    let launch = super::launch(id(QUEUE), source);
    queues.replace(&hub, 1, launch).unwrap();
    for n in 0..40 {
        queues.apply(&hub, 1, &id(QUEUE), id(&edit_id(n)), Edit::Next).unwrap();
    }
    let accounts = queues.lock();
    let done = &accounts[&1].done;
    assert_eq!(done.len(), 32);
    assert_eq!(done.front().unwrap().0, id(&edit_id(8)), "the oldest go first");
    assert_eq!(done.back().unwrap().0, id(&edit_id(39)));
}

#[tokio::test]
async fn an_edit_for_another_queue_or_with_a_gone_anchor_is_a_conflict_and_changes_nothing() {
    let (_temp, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, TOKEN).await;
    post(&app, TOKEN, start(QUEUE, "1", &["1", "2"])).await;
    next_of(&mut socket, "queueReplaced").await;

    let (status, problem) = send(&app, TOKEN, OTHER, 1, json!({ "type": "next" })).await;
    assert_eq!((status, &problem["code"]), (StatusCode::CONFLICT, &json!("queue_changed")));

    let gone = json!({ "type": "before", "item": format!("{KEY}.9") });
    let (status, problem) = send(&app, TOKEN, QUEUE, 2, add(ADDED, &["1"], gone)).await;
    assert_eq!((status, &problem["code"]), (StatusCode::CONFLICT, &json!("anchor_gone")));

    let snapshot = get(&app, TOKEN).await.1;
    assert_eq!(snapshot["version"], 0);
    assert_eq!(snapshot["state"]["sources"].as_array().unwrap().len(), 1);
    assert!(hears_no_queue(&mut socket).await);
    // The refused edit was not remembered as applied: with its anchor there, it is.
    let anchor = json!({ "type": "before", "item": format!("{KEY}.1") });
    let (status, applied) = send(&app, TOKEN, QUEUE, 2, add(ADDED, &["1"], anchor)).await;
    assert_eq!((status, applied), (StatusCode::OK, json!({ "version": 1 })));
}

#[tokio::test]
async fn an_edit_without_a_queue_is_not_found() {
    let (_temp, app) = setup().await;
    let (status, problem) = send(&app, TOKEN, QUEUE, 1, json!({ "type": "next" })).await;
    assert_eq!((status, &problem["code"]), (StatusCode::NOT_FOUND, &json!("not_found")));
}

#[tokio::test]
async fn an_add_by_query_is_sent_as_the_tracks_it_found() {
    let (_temp, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, TOKEN).await;
    post(&app, TOKEN, start(QUEUE, "1", &["1"])).await;

    let query = json!({ "type": "tracks", "albumId": "101", "sort": "album" });
    let source = json!({ "key": ADDED, "libraryId": "1", "query": query });
    let edit = json!({ "type": "add", "before": { "type": "contextEnd" }, "source": source });
    assert_eq!(send(&app, TOKEN, QUEUE, 1, edit).await.0, StatusCode::OK);

    let message = next_of(&mut socket, "queueEdit").await;
    let source = &message["edit"]["source"];
    assert_eq!(source["key"], ADDED);
    assert_eq!(source.get("query"), None);
    let mut tracks: Vec<_> = source["tracks"].as_array().unwrap().iter().collect();
    tracks.sort_by_key(|track| track.as_str().unwrap().to_owned());
    assert_eq!(tracks, [&json!("1"), &json!("2")], "the album's tracks");
    let sources = get(&app, TOKEN).await.1["state"]["sources"].clone();
    assert_eq!(sources[1]["tracks"], source["tracks"]);
}

#[tokio::test]
async fn an_add_of_tracks_out_of_reach_is_refused_as_a_start_is() {
    let (_temp, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, "user2").await;
    post(&app, "user2", start(QUEUE, "1", &["1"])).await;
    next_of(&mut socket, "queueReplaced").await;
    let refused = post_body(&app, start(OTHER, "2", &["3"])).await;

    // Library 2 is not reached by user 2, and 99 is nowhere.
    for (library, tracks) in [("2", ["3"]), ("1", ["99"]), ("9", ["3"])] {
        let mut edit = add(ADDED, &tracks, json!({ "type": "manualEnd" }));
        edit["source"]["libraryId"] = json!(library);
        let (status, problem) = send(&app, "user2", QUEUE, 1, edit).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{library} {tracks:?}");
        assert_eq!(problem["detail"], refused);
    }
    assert_eq!(get(&app, "user2").await.1["version"], 0);
    assert!(hears_no_queue(&mut socket).await);
}

/// The detail of the refusal of the start `body` by user 2.
async fn post_body(app: &Router, body: Value) -> Value {
    let (status, problem) = call(app, "user2", "POST", "/api/v1/me/queue", Some(body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    problem["detail"].clone()
}

#[tokio::test]
async fn an_edit_the_core_refuses_or_the_server_cannot_read_is_unprocessable() {
    let (_temp, app) = setup().await;
    post(&app, TOKEN, start(QUEUE, "1", &["1", "2"])).await;
    let manual = json!({ "type": "manualEnd" });
    // A key the core keeps for itself, which is no UUID either.
    let reserved = add("~0", &["1"], manual.clone());
    // A key already in the queue.
    let duplicate = add(KEY, &["1"], manual.clone());
    let neither =
        json!({ "type": "add", "before": manual, "source": { "key": OTHER, "libraryId": "1" } });
    for edit in [reserved, duplicate, neither, json!({ "type": "teleport" })] {
        let (status, _) = send(&app, TOKEN, QUEUE, 1, edit.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{edit}");
    }
    assert_eq!(get(&app, TOKEN).await.1["version"], 0);
}

#[tokio::test]
async fn a_start_tells_the_accounts_connections_to_fetch_the_queue_once() {
    let (_temp, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, TOKEN).await;
    let mut other_account = connect_with(address, "user3").await;
    let first = start(QUEUE, "1", &["1", "2"]);
    post(&app, TOKEN, first.clone()).await;
    let message = next_of(&mut socket, "queueReplaced").await;
    assert_eq!(message, json!({ "type": "queueReplaced", "queueId": QUEUE }));

    // Sent again, it changes nothing, so says nothing: the next to come is the next start's.
    post(&app, TOKEN, first).await;
    post(&app, TOKEN, start(OTHER, "1", &["2"])).await;
    assert_eq!(next_of(&mut socket, "queueReplaced").await["queueId"], OTHER);
    assert!(hears_no_queue(&mut other_account).await);
}
