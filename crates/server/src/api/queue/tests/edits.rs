//! Editing the queue: `editQueue`, and what it sends the account's connections.

use axum::Router;
use axum::http::StatusCode;
use futures_util::SinkExt;
use jewelcase_core::QueueState;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::{KEY, KEY2, OTHER, QUEUE, etag, get, setup, snapshot, start, start_body};
use crate::api::testing::socket::{Socket, connect_with, next, next_of, serve};
use crate::api::testing::{TOKEN, call, conn};
use crate::queue::list_hash;

/// The key of a source an edit adds.
const ADDED: &str = "0b1b1b1b-3b7e-4d43-9a8e-0d1c2b3a4f6f";

/// An edit ID, one for each `n`.
fn edit_id(n: usize) -> String {
    format!("7e0d1c52-3b7e-4d43-9a8e-0d1c2b3a{n:04x}")
}

/// A source key, one for each `n`.
fn key(n: usize) -> String {
    format!("cccccccc-cccc-4ccc-8ccc-{n:012x}")
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

/// The manual-end anchor.
fn manual_end() -> Value {
    json!({ "type": "manualEnd" })
}

/// Asserts that `socket` hears no queue message before it answers a ping, which it does after
/// anything the server sent first.
async fn hears_no_queue(socket: &mut Socket) {
    socket.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    loop {
        let message = next(socket).await;
        assert!(!message["type"].as_str().unwrap().starts_with("queue"), "{message}");
        if message["type"] == "pong" {
            return;
        }
    }
}

#[tokio::test]
async fn edit_broadcasts_with_hash() {
    let (temp, state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut here = connect_with(address, TOKEN).await;
    let mut there = connect_with(address, TOKEN).await;
    let mut stranger = connect_with(address, "user2").await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    // The start is saved, so only the edit can mark the account changed.
    state.queues.save(&state.db).await;

    // The client's own `list` and `length` are ignored: the server's replace them.
    let mut edit = add(ADDED, &["2", "1", "2"], manual_end());
    edit["source"]["list"] = json!("fake");
    edit["source"]["length"] = json!(99);
    let (status, answer) = send(&app, TOKEN, QUEUE, 1, edit).await;
    let list = list_hash(&[2, 1, 2]);
    let source = json!({ "key": ADDED, "libraryId": "1", "list": list, "length": 3 });
    assert_eq!(
        (status, &answer),
        (StatusCode::OK, &json!({ "version": 1, "source": source.clone() }))
    );
    for socket in [&mut here, &mut there] {
        let sent = json!({
            "type": "queueEdit", "queueId": QUEUE, "version": 1, "editId": edit_id(1),
            "edit": { "type": "add", "source": source.clone(), "before": manual_end() },
        });
        assert_eq!(next_of(socket, "queueEdit").await, sent);
    }
    hears_no_queue(&mut stranger).await;

    let queue = snapshot(&app).await;
    assert_eq!(queue["version"], 1);
    assert_eq!(queue["state"]["sources"][1], source);
    assert_eq!(
        queue["state"]["manual"],
        json!([{ "view": { "key": ADDED }, "start": 0, "len": 3 }])
    );
    assert_eq!(queue["state"]["adjusted"], true);

    // The list is held and the account marked changed: the next save stores both.
    state.queues.save(&state.db).await;
    let conn = conn(&temp);
    let length: i64 = conn
        .query_row("SELECT length FROM queue_lists WHERE user_id = 1 AND hash = ?1", [&list], |r| {
            r.get(0)
        })
        .unwrap();
    let version: i64 =
        conn.query_row("SELECT version FROM queues WHERE user_id = 1", [], |r| r.get(0)).unwrap();
    assert_eq!((length, version), (3, 1));
}

#[tokio::test]
async fn an_add_by_recipe_is_resolved_by_the_server() {
    let (_temp, _state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1"], 0)).await;
    let recipe = json!({ "type": "album", "albumId": "101", "sort": "name", "order": "asc" });
    let edit = json!({
        "type": "add", "before": manual_end(),
        "source": { "key": ADDED, "libraryId": "1", "recipe": recipe },
    });
    let (status, answer) = send(&app, TOKEN, QUEUE, 1, edit).await;
    let source = json!({
        "key": ADDED, "libraryId": "1", "list": list_hash(&[1, 2]), "length": 2, "recipe": recipe,
    });
    assert_eq!((status, &answer), (StatusCode::OK, &json!({ "version": 1, "source": source })));

    // Both or neither is refused, and so is a recipe of a library out of reach.
    let both = json!({
        "type": "add", "before": manual_end(),
        "source": { "key": KEY2, "libraryId": "1", "recipe": recipe, "tracks": ["1"] },
    });
    let neither = json!({
        "type": "add", "before": manual_end(), "source": { "key": KEY2, "libraryId": "1" },
    });
    // A core-shaped source, with a list and no tracks, is not an add the server will take.
    let by_hash = json!({
        "type": "add", "before": manual_end(),
        "source": { "key": KEY2, "libraryId": "1", "list": list_hash(&[1]), "length": 1 },
    });
    for (n, edit) in [both, neither, by_hash].into_iter().enumerate() {
        let (status, _) = send(&app, TOKEN, QUEUE, n + 2, edit).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let unreached = json!({
        "type": "add", "before": manual_end(),
        "source": { "key": KEY2, "libraryId": "2", "recipe": { "type": "library", "filters": {}, "sort": "name", "order": "asc" } },
    });
    let (status, _) = send(&app, "user2", QUEUE, 9, unreached).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "refused before the queue is looked at");
    assert_eq!(snapshot(&app).await["version"], 1);
}

#[tokio::test]
async fn non_uuid_key_refused() {
    let (_temp, _state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    let item = json!(format!("{KEY}.0"));
    let play = json!({ "play": 0, "item": item });
    for (n, edit) in [
        add("not-a-uuid", &["1"], manual_end()),
        json!({ "type": "previous", "key": "k" }),
        json!({ "type": "requeue", "play": play, "key": "k" }),
        json!({ "type": "rewind", "play": play, "key": "k" }),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, problem) = send(&app, TOKEN, QUEUE, n, edit.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{edit}: {problem}");
        assert_eq!(problem["code"], "validation_failed");
    }
    assert_eq!(snapshot(&app).await["version"], 0, "nothing applied");
}

#[tokio::test]
async fn a_used_key_is_refused_and_changes_nothing() {
    let (_temp, _state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    let (status, problem) = send(&app, TOKEN, QUEUE, 1, add(KEY, &["1"], manual_end())).await;
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::UNPROCESSABLE_ENTITY, &json!("duplicate_key"))
    );
    assert_eq!(snapshot(&app).await["version"], 0);
}

#[tokio::test]
async fn stale_queue_edit_refused() {
    let (_temp, _state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, TOKEN).await;
    // The first queue is adjusted, so the second start keeps it.
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    send(&app, TOKEN, QUEUE, 1, add(ADDED, &["2"], manual_end())).await;
    start(&app, TOKEN, start_body(OTHER, KEY2, &["2"], 0)).await;
    next_of(&mut socket, "queueReplaced").await;
    next_of(&mut socket, "queueEdit").await;
    next_of(&mut socket, "queueReplaced").await;

    // The replaced queue is parked as kept, and an edit to it, or to one that never was, is a
    // conflict that applies and sends nothing.
    for (n, queue) in [QUEUE, "33333333-3333-4333-8333-333333333333"].into_iter().enumerate() {
        let (status, problem) = send(&app, TOKEN, queue, n + 2, json!({ "type": "next" })).await;
        assert_eq!((status, &problem["code"]), (StatusCode::CONFLICT, &json!("queue_changed")));
    }
    assert_eq!(snapshot(&app).await["version"], 0);
    hears_no_queue(&mut socket).await;

    // Before any queue there is nothing to edit.
    let (status, problem) = send(&app, "user2", QUEUE, 9, json!({ "type": "next" })).await;
    assert_eq!((status, &problem["code"]), (StatusCode::NOT_FOUND, &json!("not_found")));
}

#[tokio::test]
async fn a_gone_anchor_is_a_conflict_that_holds_no_list() {
    let (temp, state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    let gone = json!({ "type": "before", "item": format!("{KEY}.9") });
    let (status, problem) = send(&app, TOKEN, QUEUE, 1, add(ADDED, &["1"], gone)).await;
    assert_eq!((status, &problem["code"]), (StatusCode::CONFLICT, &json!("anchor_gone")));
    assert_eq!(snapshot(&app).await["version"], 0);
    state.queues.save(&state.db).await;
    let lists: i64 =
        conn(&temp).query_row("SELECT count(*) FROM queue_lists", [], |r| r.get(0)).unwrap();
    assert_eq!(lists, 1, "only the start's list");
}

#[tokio::test]
async fn add_refuses_unreachable_tracks() {
    let (_temp, _state, app) = setup().await;
    start(&app, "user2", start_body(QUEUE, KEY, &["1"], 0)).await;
    let in_library = |library: &str, tracks: &[&str]| {
        let mut edit = add(ADDED, tracks, manual_end());
        edit["source"]["libraryId"] = json!(library);
        edit
    };
    // Track 3 is in library 2, which user 2 does not reach; track 99 and library 9 are not
    // there at all. The refusals say nothing of which.
    let (status, unreachable) = send(&app, "user2", QUEUE, 1, in_library("2", &["3"])).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unreachable}");
    let (_, missing) = send(&app, "user2", QUEUE, 2, in_library("2", &["99"])).await;
    let (_, missing_library) = send(&app, "user2", QUEUE, 3, in_library("9", &["99"])).await;
    assert_eq!((&unreachable, &unreachable), (&missing, &missing_library));
    let recipe = json!({ "type": "library", "filters": {}, "sort": "name", "order": "asc" });
    let by_recipe = json!({
        "type": "add", "before": manual_end(),
        "source": { "key": ADDED, "libraryId": "2", "recipe": recipe },
    });
    let (status, answer) = send(&app, "user2", QUEUE, 4, by_recipe).await;
    assert_eq!((status, &answer), (StatusCode::UNPROCESSABLE_ENTITY, &unreachable.clone()));
    for edit in [in_library("1", &["3"]), in_library("1", &["1", "99"]), in_library("1", &["01"])] {
        let (status, _) = send(&app, "user2", QUEUE, 5, edit.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{edit}");
    }
    let (_, queue) = call(&app, "user2", "GET", "/api/v1/me/queue", None).await;
    assert_eq!(queue["version"], 0, "nothing applied");

    // The owner reaches every library.
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1"], 0)).await;
    let (status, _) = send(&app, TOKEN, QUEUE, 6, in_library("2", &["3"])).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edits_apply_in_server_order() {
    let (_temp, _state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut sockets = [connect_with(address, TOKEN).await, connect_with(address, TOKEN).await];
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    let before: QueueState = serde_json::from_value(snapshot(&app).await["state"].clone()).unwrap();

    // Edits sent at once, each to the front of the Manual Queue, so their order shows.
    const N: usize = 24;
    let sends = (0..N).map(|n| {
        let app = app.clone();
        tokio::spawn(async move {
            let edit = add(&key(n), &["1"], json!({ "type": "manualStart" }));
            send(&app, TOKEN, QUEUE, n, edit).await
        })
    });
    let mut versions = Vec::new();
    for sent in sends.collect::<Vec<_>>() {
        let (status, answer) = sent.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{answer}");
        versions.push(answer["version"].as_u64().unwrap());
    }
    versions.sort_unstable();
    assert_eq!(versions, (1..=N as u64).collect::<Vec<_>>(), "one order, no gaps");

    // Every device hears the edits in version order, the same on each, and applying them in that
    // order through the core gives the queue the server holds.
    let mut heard = Vec::new();
    for socket in &mut sockets {
        let mut messages = Vec::new();
        for version in 1..=N as u64 {
            let message = next_of(socket, "queueEdit").await;
            assert_eq!(message["version"], version);
            messages.push(message);
        }
        heard.push(messages);
    }
    assert_eq!(heard[0], heard[1]);
    let mut replayed = before;
    for message in &heard[0] {
        let edit = serde_json::from_value(message["edit"].clone()).unwrap();
        replayed.apply(&edit).unwrap();
    }
    let queue = snapshot(&app).await;
    assert_eq!(queue["version"], N);
    assert_eq!(serde_json::to_value(&replayed).unwrap(), queue["state"]);
}

#[tokio::test]
async fn an_edit_sent_again_is_applied_once_and_answered_the_same() {
    let (_temp, _state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, TOKEN).await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;

    let first = add(ADDED, &["2"], manual_end());
    let answered = send(&app, TOKEN, QUEUE, 1, first.clone()).await;
    assert_eq!(answered.0, StatusCode::OK);
    send(&app, TOKEN, QUEUE, 2, json!({ "type": "next" })).await;
    assert_eq!(send(&app, TOKEN, QUEUE, 1, first).await, answered);

    let queue = snapshot(&app).await;
    assert_eq!(queue["version"], 2);
    assert_eq!(queue["state"]["sources"].as_array().unwrap().len(), 2);
    next_of(&mut socket, "queueEdit").await;
    assert_eq!(next_of(&mut socket, "queueEdit").await["version"], 2);
    hears_no_queue(&mut socket).await;
}

#[tokio::test]
async fn every_core_edit_applies_and_reaches_the_account() {
    let (_temp, _state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, TOKEN).await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2", "1", "2"], 0)).await;
    let item = |n: u64| format!("{KEY}.{n}");

    let edits = [
        add(ADDED, &["2"], manual_end()),
        json!({ "type": "move", "items": [item(3)], "before": { "type": "manualStart" } }),
        json!({ "type": "remove", "items": [item(2)] }),
        json!({ "type": "jump", "item": item(1) }),
        json!({ "type": "next" }),
        json!({ "type": "previous", "key": key(1) }),
        json!({ "type": "shuffle", "on": true, "seed": 5 }),
        json!({ "type": "repeat", "mode": "all" }),
    ];
    for (n, edit) in edits.into_iter().enumerate() {
        let (status, answer) = send(&app, TOKEN, QUEUE, n, edit.clone()).await;
        assert_eq!(status, StatusCode::OK, "{edit}: {answer}");
        assert_eq!(answer["version"], n + 1);
        let message = next_of(&mut socket, "queueEdit").await;
        assert_eq!(
            (message["version"].clone(), message["editId"].clone()),
            (json!(n + 1), json!(edit_id(n)))
        );
        if n > 0 {
            assert_eq!(message["edit"], edit);
        }
    }
    let queue = snapshot(&app).await;
    assert_eq!(queue["version"], 8);
    assert_eq!(queue["state"]["repeat"], "all");
}

#[tokio::test]
async fn edits_are_per_account() {
    let (_temp, _state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut stranger = connect_with(address, "user2").await;
    // Both accounts use the same queue ID and key, as two devices' random ones may never but
    // a client may.
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    start(&app, "user2", start_body(QUEUE, KEY, &["2"], 0)).await;
    next_of(&mut stranger, "queueReplaced").await;

    let (status, _) = send(&app, TOKEN, QUEUE, 1, add(ADDED, &["1"], manual_end())).await;
    assert_eq!(status, StatusCode::OK);
    hears_no_queue(&mut stranger).await;
    let (_, theirs) = call(&app, "user2", "GET", "/api/v1/me/queue", None).await;
    assert_eq!(
        (theirs["version"].clone(), theirs["state"]["manual"].clone()),
        (json!(0), json!([]))
    );

    // Nor does an edit id of one account stand for another's.
    let (status, answer) = send(&app, "user2", QUEUE, 1, add(ADDED, &["2"], manual_end())).await;
    assert_eq!((status, &answer["version"]), (StatusCode::OK, &json!(1)));
    let (_, theirs) = call(&app, "user2", "GET", "/api/v1/me/queue", None).await;
    assert_eq!(theirs["state"]["sources"][1]["length"], 1);
    assert_eq!(snapshot(&app).await["state"]["sources"][1]["length"], 1);
}

#[tokio::test]
async fn an_add_may_be_far_larger_than_axums_default_body() {
    let (_temp, state, app) = setup().await;
    // A track with a 19-digit ID, as real ones have, so each place costs 22 bytes of JSON.
    let big = 4_000_000_000_000_000_000_i64;
    state
        .db
        .write(move |tx| {
            crate::db::testing::Track {
                title: "A".into(),
                sort_key: b"a".to_vec(),
                ..crate::db::testing::Track::new(big, 1, 101)
            }
            .insert(tx)
        })
        .await
        .unwrap();
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1"], 0)).await;
    let id = big.to_string();
    let tracks = vec![id.as_str(); 200_000];
    let edit = add(ADDED, &tracks, manual_end());
    assert!(edit.to_string().len() > 2 << 20, "past axum's default limit");
    let (status, answer) = send(&app, TOKEN, QUEUE, 1, edit).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["source"]["length"], 200_000);
    assert_eq!(answer["source"]["list"], json!(list_hash(&vec![big.cast_unsigned(); 200_000])));
}

#[tokio::test]
async fn keys_are_stored_lowercase() {
    let (_temp, _state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    let upper = KEY2.to_uppercase();
    send(&app, TOKEN, QUEUE, 1, json!({ "type": "next" })).await;
    let play = json!({ "play": 0, "item": format!("{KEY}.0") });
    let requeue = json!({ "type": "requeue", "play": play, "key": upper });
    let (status, answer) = send(&app, TOKEN, QUEUE, 2, requeue).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    // The copy's key is the lowercase one, so the same key in lowercase is already used.
    let (status, problem) = send(&app, TOKEN, QUEUE, 3, add(KEY2, &["1"], manual_end())).await;
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::UNPROCESSABLE_ENTITY, &json!("duplicate_key"))
    );
}

#[tokio::test]
async fn an_edit_moves_the_etag_to_the_next_version() {
    let (_temp, _state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    let (_, headers, _) = get(&app, &[]).await;
    let tag = etag(&headers);
    let run = tag.trim_start_matches('"').split('.').next().unwrap();
    send(&app, TOKEN, QUEUE, 1, json!({ "type": "next" })).await;
    // The tag a device builds for the version it reached by hearing the edit.
    let (status, ..) = get(&app, &[("if-none-match", &format!("\"{run}.{QUEUE}.1\""))]).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn repeat_survives_a_start() {
    let (_temp, _state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    let (status, _) = send(&app, TOKEN, QUEUE, 1, json!({ "type": "repeat", "mode": "all" })).await;
    assert_eq!(status, StatusCode::OK);
    start(&app, TOKEN, start_body(OTHER, KEY2, &["2", "1"], 0)).await;
    assert_eq!(snapshot(&app).await["state"]["repeat"], "all");
}
