//! Kept queues: `listKeptQueues`, `restoreQueue` and `deleteKeptQueue`, and `queuesChanged`.

use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::http::StatusCode;
use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::{KEY, KEY2, OTHER, QUEUE, message, setup, snapshot, start, start_body};
use crate::api::testing::player::report;
use crate::api::testing::socket::{Socket, connect_with, next, serve};
use crate::api::testing::{TOKEN, call, conn};

/// Edit IDs are one for each adjustment, since a retried edit ID is answered, not applied.
static EDITS: AtomicUsize = AtomicUsize::new(1);

/// A queue ID, one for each `n`.
fn queue(n: usize) -> String {
    format!("{n:08x}-1111-4111-8111-111111111111")
}

/// A source key, one for each `n`.
fn key(n: usize) -> String {
    format!("cccccccc-cccc-4ccc-8ccc-{n:012x}")
}

/// Adds a track to `queue`'s Manual Queue as the owner, which makes the queue adjusted, so a
/// start keeps it.
async fn adjust(app: &Router, queue: &str, n: usize) {
    let body = json!({
        "editId": format!("7e0d1c52-3b7e-4d43-9a8e-0d1c2b3a{n:04x}"), "queueId": queue,
        "edit": {
            "type": "add", "before": { "type": "manualEnd" },
            "source": { "key": format!("dddddddd-dddd-4ddd-8ddd-{n:012x}"), "libraryId": "1", "tracks": ["2"] },
        },
    });
    let (status, answer) = call(app, TOKEN, "POST", "/api/v1/me/queue/edits", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
}

/// Starts `queue` over tracks 1 and 2 of library 1, and adjusts it if `adjusted`, by an edit
/// that is the account's own `n`th.
async fn begin(app: &Router, queue: &str, key: &str, adjusted: bool) {
    let (status, _) = start(app, TOKEN, start_body(queue, key, &["1", "2"], 0)).await;
    assert_eq!(status, StatusCode::OK);
    if adjusted {
        adjust(app, queue, EDITS.fetch_add(1, Ordering::Relaxed)).await;
    }
}

/// `listKeptQueues` as `token` with `query`.
async fn kept(app: &Router, token: &str, query: &str) -> (StatusCode, Value) {
    call(app, token, "GET", &format!("/api/v1/me/queues?{query}"), None).await
}

/// The queue IDs of a page's items.
fn ids(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["queueId"].as_str().unwrap().to_owned())
        .collect()
}

/// Asserts that `socket` hears no `queuesChanged` before it answers a ping.
async fn hears_no_change(socket: &mut Socket) {
    socket.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    loop {
        let message = next(socket).await;
        assert_ne!(message["type"], "queuesChanged", "{message}");
        if message["type"] == "pong" {
            return;
        }
    }
}

#[tokio::test]
async fn kept_queue_listed_before_save() {
    let (_temp, state, app) = setup().await;
    begin(&app, QUEUE, KEY, true).await;
    begin(&app, OTHER, KEY2, false).await;
    begin(&app, &queue(3), &key(3), false).await;

    // Nothing is saved: the queue just kept is in memory, and OTHER, parked unadjusted, is not.
    let (status, page) = kept(&app, TOKEN, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&page), [QUEUE]);
    assert_eq!((&page["nextCursor"], &page["total"]), (&Value::Null, &json!(1)));
    let item = &page["items"][0];
    assert_eq!(item["context"], json!({ "type": "album", "id": "101" }));
    assert_eq!(item["current"], json!({ "libraryId": "1", "trackId": "1" }));
    assert_eq!((&item["upNext"], &item["history"]), (&json!(2), &json!(0)));
    assert!(item["lastActive"].as_str().unwrap().ends_with('Z'), "{item}");
    let (_, theirs) = kept(&app, "user2", "").await;
    assert_eq!((&theirs["items"], &theirs["total"]), (&json!([]), &json!(0)));

    // Saved, it is listed once, and its track is read from the stored list.
    state.queues.save(&state.db).await;
    let (_, saved) = kept(&app, TOKEN, "").await;
    assert_eq!((ids(&saved), &saved["total"]), (vec![QUEUE.to_owned()], &json!(1)));
    assert_eq!(saved["items"][0]["current"], json!({ "libraryId": "1", "trackId": "1" }));
}

#[tokio::test]
async fn restore_returns_position() {
    let (_temp, state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut here = connect_with(address, TOKEN).await;
    let mut there = connect_with(address, TOKEN).await;
    begin(&app, QUEUE, KEY, true).await;
    report(
        &mut here,
        "paused",
        json!({ "itemId": format!("{KEY}.0"), "positionMs": 4200, "start": true }),
    )
    .await;
    begin(&app, OTHER, KEY2, true).await;
    state.queues.save(&state.db).await;
    for socket in [&mut here, &mut there] {
        // OTHER's adjustment is the last thing sent before the restore.
        while message(socket).await["queueId"] != json!(OTHER) {}
        while message(socket).await["type"] != "queueEdit" {}
    }

    let (status, answer) =
        call(&app, TOKEN, "POST", &format!("/api/v1/me/queues/{QUEUE}/restore"), None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["queueId"], json!(QUEUE));
    assert_eq!(answer["position"], json!({ "item": format!("{KEY}.0"), "positionMs": 4200 }));
    assert_eq!(answer["state"]["sources"][0]["key"], json!(KEY));
    for socket in [&mut here, &mut there] {
        assert_eq!(message(socket).await, json!({ "type": "queueReplaced", "queueId": QUEUE }));
        assert_eq!(message(socket).await, json!({ "type": "queuesChanged" }));
    }

    // The position is for that one answer, and the queue the restore replaced is kept.
    let active = snapshot(&app).await;
    assert_eq!(active["queueId"], json!(QUEUE));
    assert!(active.get("position").is_none(), "{active}");
    assert_eq!(ids(&kept(&app, TOKEN, "").await.1), [OTHER]);
    // A position is never paired with another queue's snapshot.
    let at = crate::queue::Position { item: format!("{KEY}.0"), position_ms: 1 };
    let (_, body) =
        state.queues.snapshot(1, None, Some((&OTHER.to_owned().try_into().unwrap(), &at))).unwrap();
    assert!(serde_json::from_slice::<Value>(&body.unwrap()).unwrap().get("position").is_none());
    // The player state is left alone.
    let (_, player) = call(&app, TOKEN, "GET", "/api/v1/me/player", None).await;
    assert_eq!(player["itemId"], json!(format!("{KEY}.0")));
}

#[tokio::test]
async fn restore_of_the_active_queue_is_refused() {
    let (_temp, _state, app) = setup().await;
    begin(&app, QUEUE, KEY, true).await;
    let (status, body) =
        call(&app, TOKEN, "POST", &format!("/api/v1/me/queues/{QUEUE}/restore"), None).await;
    assert_eq!((status, &body["code"]), (StatusCode::CONFLICT, &json!("queue_active")));
}

#[tokio::test]
async fn delete_active_refused() {
    let (_temp, state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, TOKEN).await;
    begin(&app, QUEUE, KEY, true).await;
    let (status, body) =
        call(&app, TOKEN, "DELETE", &format!("/api/v1/me/queues/{QUEUE}"), None).await;
    assert_eq!((status, &body["code"]), (StatusCode::CONFLICT, &json!("queue_active")));
    assert_eq!(snapshot(&app).await["queueId"], json!(QUEUE));

    // Parked, even before the save writes it, it is no longer the active one: it goes.
    begin(&app, OTHER, KEY2, false).await;
    // The start of OTHER is the last thing heard before the delete.
    while message(&mut socket).await != json!({ "type": "queueReplaced", "queueId": OTHER }) {}
    assert_eq!(message(&mut socket).await, json!({ "type": "queuesChanged" }));
    let (status, body) =
        call(&app, TOKEN, "DELETE", &format!("/api/v1/me/queues/{QUEUE}"), None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    assert_eq!(message(&mut socket).await, json!({ "type": "queuesChanged" }));
    state.queues.save(&state.db).await;
    assert_eq!(kept(&app, TOKEN, "").await.1["items"], json!([]));
    let (status, _) =
        call(&app, TOKEN, "POST", &format!("/api/v1/me/queues/{QUEUE}/restore"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(snapshot(&app).await["queueId"], json!(OTHER));
}

#[tokio::test]
async fn kept_queues_are_per_account() {
    let (_temp, state, app) = setup().await;
    begin(&app, QUEUE, KEY, true).await;
    begin(&app, OTHER, KEY2, false).await;
    state.queues.save(&state.db).await;
    let address = serve(app.clone()).await;
    let mut stranger = connect_with(address, "user2").await;

    let (_, theirs) = kept(&app, "user2", "").await;
    assert_eq!((&theirs["items"], &theirs["total"]), (&json!([]), &json!(0)));
    for (method, path) in [("DELETE", String::new()), ("POST", "/restore".to_owned())] {
        let uri = format!("/api/v1/me/queues/{QUEUE}{path}");
        let (status, body) = call(&app, "user2", method, &uri, None).await;
        assert_eq!((status, &body["code"]), (StatusCode::NOT_FOUND, &json!("not_found")));
    }
    // An unknown queue, or a malformed ID, is the same.
    for id in [queue(9), "nope".to_owned()] {
        let (status, _) =
            call(&app, TOKEN, "DELETE", &format!("/api/v1/me/queues/{id}"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    hears_no_change(&mut stranger).await;
    assert_eq!(ids(&kept(&app, TOKEN, "").await.1), [QUEUE]);
    assert_eq!(snapshot(&app).await["queueId"], json!(OTHER));
}

#[tokio::test]
async fn kept_queues_page_by_keyset() {
    let (temp, state, app) = setup().await;
    for n in 1..=4 {
        begin(&app, &queue(n), &key(n), n < 4).await;
    }
    let all = |pages: Vec<Value>| -> Vec<String> { pages.iter().flat_map(ids).collect() };
    let page_through = async |query: &str| {
        let mut pages = Vec::new();
        let mut cursor = String::new();
        loop {
            let (status, page) = kept(&app, TOKEN, &format!("limit=2{query}{cursor}")).await;
            assert_eq!(status, StatusCode::OK, "{page}");
            // Every page counts the whole list.
            assert_eq!(page["total"], 3, "{page}");
            let next = page["nextCursor"].as_str().map(str::to_owned);
            pages.push(page);
            match next {
                Some(next) => cursor = format!("&cursor={next}"),
                None => return pages,
            }
        }
    };

    // Not yet saved, three kept queues in memory page two and one.
    let pages = page_through("").await;
    assert_eq!(pages.len(), 2);
    let unsaved = all(pages);
    assert_eq!(unsaved.len(), 3);

    // Saved, they share a time, and the page boundary falls between equal ones.
    state.queues.save(&state.db).await;
    conn(&temp).execute("UPDATE queues SET last_active = 1000 WHERE active = 0", []).unwrap();
    let pages = page_through("").await;
    assert_eq!(
        pages.iter().map(|page| page["items"].as_array().unwrap().len()).collect::<Vec<_>>(),
        [2, 1]
    );
    let saved = all(pages);
    assert_eq!(saved, [queue(3), queue(2), queue(1)]);
    let mut sorted = unsaved.clone();
    sorted.sort();
    assert_eq!(unsaved.iter().rev().cloned().collect::<Vec<_>>(), sorted);

    let (status, body) = kept(&app, TOKEN, "cursor=nonsense").await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::UNPROCESSABLE_ENTITY, &json!("validation_failed"))
    );
    assert_eq!(kept(&app, TOKEN, "limit=0").await.0, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn an_unreadable_kept_row_is_skipped() {
    let (temp, state, app) = setup().await;
    begin(&app, QUEUE, KEY, true).await;
    begin(&app, OTHER, KEY2, true).await;
    begin(&app, &queue(3), &key(3), false).await;
    state.queues.save(&state.db).await;
    conn(&temp).execute("UPDATE queues SET state = '{}' WHERE queue_id = ?1", [QUEUE]).unwrap();

    let (status, page) = kept(&app, TOKEN, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&page), [OTHER]);
    assert_eq!(page["total"], 2, "the total counts it");
}

#[tokio::test]
async fn start_broadcasts_queues_changed() {
    let (_temp, _state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, TOKEN).await;
    let body = start_body(QUEUE, KEY, &["1", "2"], 0);
    start(&app, TOKEN, body.clone()).await;
    assert_eq!(message(&mut socket).await["type"], "queueReplaced");
    assert_eq!(message(&mut socket).await, json!({ "type": "queuesChanged" }));

    // A retry changes nothing, and says nothing.
    start(&app, TOKEN, body).await;
    hears_no_change(&mut socket).await;
}
