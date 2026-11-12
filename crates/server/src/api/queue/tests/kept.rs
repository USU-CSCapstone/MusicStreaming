//! Kept queues: `listKeptQueues`, `getKeptQueue`, `restoreQueue` and `deleteKeptQueue`, and
//! `queuesChanged`.

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

/// `restoreQueue` as the owner: the answer's `position`.
async fn restored_position(app: &Router, queue: &str) -> Value {
    let (status, answer) =
        call(app, TOKEN, "POST", &format!("/api/v1/me/queues/{queue}/restore"), None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer["position"].clone()
}

/// A start or restore parks the active queue with the item and point the player state names
/// under the queues' lock, end to end through the API: while playing, the point is as far as it
/// has played; the account's own state is read, never another's; and a state naming no item of
/// the queue leaves the position the queue had.
#[tokio::test]
async fn parking_takes_its_place_from_the_player_state() {
    let (_temp, state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut stranger = connect_with(address, "user2").await;
    let item = |key: &str, n: u64| format!("{key}.{n}");
    // Another account's state, naming an item of its own queue under a key the owner uses later.
    let theirs = start_body(OTHER, KEY2, &["1", "2"], 0);
    assert_eq!(start(&app, "user2", theirs).await.0, StatusCode::OK);
    let place = json!({ "itemId": item(KEY2, 0), "positionMs": 9000, "start": true });
    report(&mut stranger, "playing", place).await;

    begin(&app, QUEUE, KEY, true).await;
    let place = json!({ "itemId": item(KEY, 1), "positionMs": 4200, "start": true });
    report(&mut desk, "playing", place).await;
    begin(&app, OTHER, KEY2, true).await;
    report(&mut desk, "paused", json!({ "itemId": item(KEY2, 0), "positionMs": 700 })).await;
    state.queues.save(&state.db).await;

    // Parked while playing: as far as it played, which is never short of where it was reported.
    let first = restored_position(&app, QUEUE).await;
    assert_eq!(first["item"], json!(item(KEY, 1)));
    let at = first["positionMs"].as_u64().unwrap();
    assert!((4200..4200 + 10_000).contains(&at), "{first}");
    // OTHER was parked by that restore, paused at its own place.
    let other = restored_position(&app, OTHER).await;
    assert_eq!(other, json!({ "item": item(KEY2, 0), "positionMs": 700 }));

    // The state still names OTHER's item, which QUEUE lacks, so parking QUEUE keeps its place.
    assert_eq!(restored_position(&app, QUEUE).await, first);
    begin(&app, &queue(3), &key(3), true).await;
    assert_eq!(restored_position(&app, QUEUE).await, first, "kept through another parking");
}

/// `getKeptQueue` as `token`.
async fn kept_one(app: &Router, token: &str, queue: &str) -> (StatusCode, Value) {
    call(app, token, "GET", &format!("/api/v1/me/queues/{queue}"), None).await
}

/// A kept queue is read as a restore would make it active, with its position, from memory before
/// the save and from the database after, and reading it restores nothing. The active queue, a
/// parked queue that was not adjusted, another account's and an unknown one are `404`.
#[tokio::test]
async fn a_kept_queue_is_read_without_restoring_it() {
    let (_temp, state, app) = setup().await;
    let address = serve(app.clone()).await;
    let mut desk = connect_with(address, TOKEN).await;
    begin(&app, QUEUE, KEY, true).await;
    let place = json!({ "itemId": format!("{KEY}.1"), "positionMs": 3000, "start": true });
    report(&mut desk, "paused", place).await;
    begin(&app, OTHER, KEY2, false).await;
    begin(&app, &queue(3), &key(3), false).await;

    let check = |answer: &Value| {
        assert_eq!(answer["queueId"], json!(QUEUE));
        assert_eq!(answer["version"], json!(1));
        assert_eq!(answer["context"], json!({ "type": "album", "id": "101" }));
        assert_eq!(answer["state"]["sources"][0]["key"], json!(KEY));
        assert_eq!(answer["state"]["adjusted"], json!(true));
        assert_eq!(answer["position"], json!({ "item": format!("{KEY}.1"), "positionMs": 3000 }));
    };
    let (status, parked) = kept_one(&app, TOKEN, QUEUE).await;
    assert_eq!(status, StatusCode::OK, "{parked}");
    check(&parked);
    // Parked unadjusted and not yet saved, it is due for deletion.
    assert_eq!(kept_one(&app, TOKEN, OTHER).await.0, StatusCode::NOT_FOUND);
    state.queues.save(&state.db).await;
    let (status, saved) = kept_one(&app, TOKEN, QUEUE).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    check(&saved);
    assert_eq!(snapshot(&app).await["queueId"], json!(queue(3)));
    assert_eq!(ids(&kept(&app, TOKEN, "").await.1), [QUEUE]);

    for (token, id) in [
        (TOKEN, queue(3)),
        (TOKEN, OTHER.to_owned()),
        ("user2", QUEUE.to_owned()),
        (TOKEN, queue(9)),
    ] {
        let (status, body) = kept_one(&app, token, &id).await;
        assert_eq!((status, &body["code"]), (StatusCode::NOT_FOUND, &json!("not_found")), "{id}");
    }
}

/// `listKeptQueues` as the owner with `query`, naming `known` in `If-None-Match`: the status, the
/// `ETag` and whether a body came.
async fn kept_tagged(
    app: &Router,
    query: &str,
    known: Option<&str>,
) -> (StatusCode, Option<String>, bool) {
    let mut request = axum::http::Request::get(format!("/api/v1/me/queues?{query}"))
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    if let Some(known) = known {
        request = request.header(axum::http::header::IF_NONE_MATCH, known);
    }
    let (status, headers, body) =
        crate::api::testing::respond(app.clone(), request.body(axum::body::Body::empty()).unwrap())
            .await;
    let tag = headers.get(axum::http::header::ETAG).map(|v| v.to_str().unwrap().to_owned());
    (status, tag, !body.is_empty())
}

/// The first page of the kept list carries a tag over the whole kept set: the same tag answers
/// `304` with no body until a queue is kept, saved, deleted or expired, and another `limit` has
/// another tag. Later pages carry none.
#[tokio::test]
async fn the_kept_list_is_not_sent_again_while_it_has_not_changed() {
    let (_temp, state, app) = setup().await;
    let first = || async { kept_tagged(&app, "limit=1", None).await.1.unwrap() };
    let unchanged = |tag: String| {
        let app = app.clone();
        async move { kept_tagged(&app, "limit=1", Some(&tag)).await }
    };
    let empty = first().await;
    assert_eq!(
        unchanged(empty.clone()).await,
        (StatusCode::NOT_MODIFIED, Some(empty.clone()), false)
    );

    begin(&app, QUEUE, KEY, true).await;
    begin(&app, OTHER, KEY2, true).await;
    let parked = first().await;
    assert_ne!(parked, empty);
    assert_eq!(unchanged(parked.clone()).await.0, StatusCode::NOT_MODIFIED);
    assert_ne!(kept_tagged(&app, "limit=2", None).await.1.unwrap(), parked);

    state.queues.save(&state.db).await;
    let saved = first().await;
    assert_ne!(saved, parked);
    let (status, tag, body) = unchanged(parked).await;
    assert_eq!((status, tag.as_deref(), body), (StatusCode::OK, Some(saved.as_str()), true));

    begin(&app, &queue(3), &key(3), true).await;
    state.queues.save(&state.db).await;
    let three = first().await;
    // A later page carries no tag.
    let (_, page) = kept(&app, TOKEN, "limit=1").await;
    let cursor = page["nextCursor"].as_str().unwrap().to_owned();
    assert_eq!(kept_tagged(&app, &format!("limit=1&cursor={cursor}"), None).await.1, None);
    let (status, _) =
        call(&app, TOKEN, "DELETE", &format!("/api/v1/me/queues/{QUEUE}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let deleted = first().await;
    assert_ne!(deleted, three);

    // Expired, the rest goes: the tag changes again.
    crate::db::retention::sweep(
        &state.db,
        crate::db::now_ms() + crate::db::retention::WINDOW_MS + 60_000,
    )
    .await;
    assert_eq!(kept(&app, TOKEN, "").await.1["items"], json!([]));
    assert_ne!(first().await, deleted);
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

/// A restore from a client that accepts gzip, sent again under its key, is answered as the first
/// was and not refused as `queue_active`: the answer is stored as the handler made it.
#[tokio::test]
async fn a_restore_repeated_under_its_key_is_answered_not_refused() {
    let (_temp, state, app) = setup().await;
    begin(&app, QUEUE, KEY, true).await;
    begin(&app, OTHER, KEY2, true).await;
    state.queues.save(&state.db).await;

    let send = || {
        let request = axum::http::Request::post(format!("/api/v1/me/queues/{QUEUE}/restore"))
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("accept-encoding", "gzip")
            .header("idempotency-key", "restore-1")
            .body(axum::body::Body::empty())
            .unwrap();
        crate::api::testing::respond(app.clone(), request)
    };
    let (status, _, first) = send().await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, second) = send().await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second, first);
}
