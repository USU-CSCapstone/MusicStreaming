//! Reading the queue: `getQueueList` and `getQueueRows`.

use std::io::Read;

use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, header};
use flate2::read::GzDecoder;
use jewelcase_core::QueueState;
use serde_json::{Value, json};

use super::{KEY, KEY2, OTHER, QUEUE, etag, setup, snapshot, start, start_body};
use crate::api::testing::{TOKEN, call, conn, owner_request, respond};
use crate::queue::list_hash;

/// The key of a source an edit adds.
const ADDED: &str = "0b1b1b1b-3b7e-4d43-9a8e-0d1c2b3a4f6f";

/// `getQueueRows` as `token`.
async fn rows(app: &Router, token: &str, query: &str) -> (StatusCode, Value) {
    call(app, token, "GET", &format!("/api/v1/me/queue/rows?{query}"), None).await
}

/// `getQueueList` as `token`.
async fn list(app: &Router, token: &str, hash: &str) -> (StatusCode, Value) {
    call(app, token, "GET", &format!("/api/v1/me/queue/lists/{hash}"), None).await
}

/// Sends `edit` as edit `n` for `queue`, as the owner, and expects it to apply.
async fn edit(app: &Router, queue: &str, n: usize, edit: Value) {
    let body = json!({ "editId": format!("7e0d1c52-3b7e-4d43-9a8e-0d1c2b3a{n:04x}"), "queueId": queue, "edit": edit });
    let (status, answer) = call(app, TOKEN, "POST", "/api/v1/me/queue/edits", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
}

/// The `(item, trackId)` of each of `rows`.
fn pairs(rows: &Value) -> Vec<(String, Value)> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| (row["item"].as_str().unwrap().to_owned(), row["trackId"].clone()))
        .collect()
}

#[tokio::test]
async fn lists_are_per_account() {
    let (_temp, state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2", "1"], 0)).await;
    let hash = list_hash(&[1, 2, 1]);
    // Held in memory and not yet saved, it is still the owner's alone.
    assert_eq!(list(&app, "user2", &hash).await.0, StatusCode::NOT_FOUND);
    state.queues.save(&state.db).await;

    let (status, body) = list(&app, TOKEN, &hash).await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, json!({ "libraryId": "1", "tracks": ["1", "2", "1"] }))
    );

    // Another account's hash answers as an unknown one does, saved or not.
    let (status, theirs) = list(&app, "user2", &hash).await;
    let (unknown_status, unknown) = list(&app, TOKEN, &"0".repeat(64)).await;
    assert_eq!((status, unknown_status), (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND));
    assert_eq!(theirs, unknown);
    start(&app, "user2", start_body(OTHER, KEY2, &["2"], 0)).await;
    assert_eq!(list(&app, "user2", &hash).await.0, StatusCode::NOT_FOUND);
    state.queues.save(&state.db).await;
    assert_eq!(list(&app, "user2", &hash).await.0, StatusCode::NOT_FOUND);
    assert_eq!(list(&app, "user2", &list_hash(&[2])).await.0, StatusCode::OK);
    assert_eq!(list(&app, TOKEN, &list_hash(&[2])).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unsaved_list_served() {
    let (temp, state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["2", "1"], 0)).await;
    let hash = list_hash(&[2, 1]);
    let stored: i64 =
        conn(&temp).query_row("SELECT count(*) FROM queue_lists", [], |r| r.get(0)).unwrap();
    assert_eq!(stored, 0, "nothing is saved yet");

    let request = owner_request("GET", &format!("/api/v1/me/queue/lists/{hash}"));
    let (status, headers, body) = respond(app.clone(), request.body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(etag(&headers), format!("\"{hash}\""));
    assert_eq!(headers[header::CACHE_CONTROL], "private, max-age=31536000, immutable");
    let memory: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(memory, json!({ "libraryId": "1", "tracks": ["2", "1"] }));

    state.queues.save(&state.db).await;
    assert_eq!(list(&app, TOKEN, &hash).await, (StatusCode::OK, memory));
}

#[tokio::test]
async fn list_is_conditional_and_gzipped() {
    let (_temp, state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2"], 0)).await;
    state.queues.save(&state.db).await;
    let hash = list_hash(&[1, 2]);
    let uri = format!("/api/v1/me/queue/lists/{hash}");
    let get = |known: Option<&str>, zip: bool| {
        let mut request = owner_request("GET", &uri);
        if let Some(known) = known {
            request = request.header(header::IF_NONE_MATCH, known);
        }
        if zip {
            request = request.header(header::ACCEPT_ENCODING, "gzip");
        }
        respond(app.clone(), request.body(Body::empty()).unwrap())
    };

    let tag = format!("\"{hash}\"");
    for known in [tag.clone(), format!("\"x\", {tag}"), "*".to_owned()] {
        let (status, headers, body) = get(Some(&known), false).await;
        assert_eq!((status, body.len()), (StatusCode::NOT_MODIFIED, 0));
        assert_eq!(etag(&headers), tag);
        assert_eq!(headers[header::CACHE_CONTROL], "private, max-age=31536000, immutable");
    }
    assert_eq!(get(Some("\"x\""), false).await.0, StatusCode::OK);

    // A tag the caller has no list for is not a 304.
    let other = format!("/api/v1/me/queue/lists/{}", "0".repeat(64));
    let request = owner_request("GET", &other).header(header::IF_NONE_MATCH, "*");
    let (status, ..) = respond(app.clone(), request.body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, headers, zipped) = get(None, true).await;
    assert_eq!(
        (status, headers[header::CONTENT_ENCODING].to_str().unwrap()),
        (StatusCode::OK, "gzip")
    );
    let mut text = String::new();
    GzDecoder::new(&zipped[..]).read_to_string(&mut text).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["tracks"], json!(["1", "2"]));
}

#[tokio::test]
async fn a_kept_queues_list_is_served() {
    let (_temp, state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "1"], 0)).await;
    // An adjusted queue is kept when replaced.
    edit(&app, QUEUE, 1, json!({ "type": "remove", "items": [format!("{KEY}.1")] })).await;
    state.queues.save(&state.db).await;
    start(&app, TOKEN, start_body(OTHER, KEY2, &["2", "2"], 0)).await;
    let (kept, active) = (list_hash(&[1, 1]), list_hash(&[2, 2]));

    // Parked and not yet saved, then kept.
    assert_eq!(list(&app, TOKEN, &kept).await.0, StatusCode::OK);
    state.queues.save(&state.db).await;
    assert_eq!(list(&app, TOKEN, &kept).await.0, StatusCode::OK);
    assert_eq!(list(&app, TOKEN, &active).await.0, StatusCode::OK);

    // A replaced queue that was not adjusted is deleted with its list.
    start(&app, TOKEN, start_body(KEY, KEY, &["1", "2"], 0)).await;
    state.queues.save(&state.db).await;
    assert_eq!(list(&app, TOKEN, &active).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rows_page_sections() {
    let (_temp, state, app) = setup().await;
    let (status, _) = rows(&app, TOKEN, "section=upNext").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2", "1", "2"], 0)).await;
    let add = json!({
        "type": "add", "before": { "type": "manualEnd" },
        "source": { "key": ADDED, "libraryId": "1", "tracks": ["2", "2"] },
    });
    edit(&app, QUEUE, 1, add).await;

    let (status, page) = rows(&app, TOKEN, "section=upNext").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (page["queueId"].clone(), page["version"].clone(), page["total"].clone()),
        (json!(QUEUE), json!(1), json!(5))
    );
    assert_eq!(
        (page["manual"].clone(), page["shuffled"].clone(), page["repeat"].clone()),
        (json!(2), json!(false), json!("off"))
    );
    assert_eq!(
        page["current"],
        json!({ "item": format!("{KEY}.0"), "libraryId": "1", "trackId": "1" })
    );
    let item = |key: &str, n: u64| format!("{key}.{n}");
    let all: Vec<(String, Value)> = vec![
        (item(ADDED, 0), json!("2")),
        (item(ADDED, 1), json!("2")),
        (item(KEY, 1), json!("2")),
        (item(KEY, 2), json!("1")),
        (item(KEY, 3), json!("2")),
    ];
    assert_eq!(pairs(&page["rows"]), all);
    // Saved, the two lists are read from the database and the page is the same.
    state.queues.save(&state.db).await;
    let (_, stored) = rows(&app, TOKEN, "section=upNext").await;
    assert_eq!(pairs(&stored["rows"]), all);
    assert!(page["rows"][0].get("play").is_none());

    let (_, middle) = rows(&app, TOKEN, "section=upNext&offset=1&limit=2").await;
    assert_eq!(pairs(&middle["rows"]), all[1..3]);
    assert_eq!(middle["total"], 5);
    let (_, past) = rows(&app, TOKEN, "section=upNext&offset=9").await;
    assert_eq!((past["rows"].clone(), past["total"].clone()), (json!([]), json!(5)));

    // The first next plays ADDED.0 and passes KEY.0 into History; the second plays ADDED.1.
    edit(&app, QUEUE, 2, json!({ "type": "next" })).await;
    // History has no Manual Queue rows, though one is still queued.
    let (_, early) = rows(&app, TOKEN, "section=history").await;
    assert_eq!(early["manual"], 0);
    edit(&app, QUEUE, 3, json!({ "type": "next" })).await;
    let (_, history) = rows(&app, TOKEN, "section=history").await;
    assert_eq!((history["total"].clone(), history["manual"].clone()), (json!(2), json!(0)));
    assert_eq!(
        history["rows"],
        json!([
            { "item": item(ADDED, 0), "libraryId": "1", "trackId": "2", "play": 1 },
            { "item": item(KEY, 0), "libraryId": "1", "trackId": "1", "play": 0 },
        ])
    );
    assert_eq!(history["current"]["item"], json!(item(ADDED, 1)));
    let (_, up_next) = rows(&app, TOKEN, "section=upNext").await;
    assert_eq!((up_next["total"].clone(), up_next["manual"].clone()), (json!(3), json!(0)));
    let (_, one) = rows(&app, TOKEN, "section=history&offset=1&limit=1").await;
    assert_eq!(one["rows"][0]["play"], 0);
}

#[tokio::test]
async fn unreadable_row_is_null() {
    let (temp, state, app) = setup().await;
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "2", "1"], 0)).await;
    state.queues.save(&state.db).await;
    let (_, before) = rows(&app, TOKEN, "section=upNext").await;
    assert_eq!(
        pairs(&before["rows"]),
        [(format!("{KEY}.1"), json!("2")), (format!("{KEY}.2"), json!("1"))]
    );

    conn(&temp).execute("DELETE FROM queue_lists", []).unwrap();
    let (status, after) = rows(&app, TOKEN, "section=upNext").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        after["rows"],
        json!([
            { "item": format!("{KEY}.1"), "libraryId": "1", "trackId": null },
            { "item": format!("{KEY}.2"), "libraryId": "1", "trackId": null },
        ])
    );
    assert_eq!(
        after["current"],
        json!({ "item": format!("{KEY}.0"), "libraryId": "1", "trackId": null })
    );
    assert_eq!(after["total"], 2);
}

#[tokio::test]
async fn rows_follow_shuffle() {
    let (_temp, state, app) = setup().await;
    let tracks: Vec<&str> =
        "12212112121221211212".chars().map(|c| if c == '1' { "1" } else { "2" }).collect();
    start(&app, TOKEN, start_body(QUEUE, KEY, &tracks, 3)).await;
    edit(&app, QUEUE, 1, json!({ "type": "shuffle", "on": true, "seed": 7 })).await;
    edit(&app, QUEUE, 2, json!({ "type": "next" })).await;
    // Rows come from memory, then from the database once saved.
    for saved in [false, true] {
        if saved {
            state.queues.save(&state.db).await;
        }
        let snapshot = snapshot(&app).await;
        let mut core: QueueState = serde_json::from_value(snapshot["state"].clone()).unwrap();
        let ids: Vec<u64> = tracks.iter().map(|t| t.parse().unwrap()).collect();
        core.attach_list(KEY, list_hash(&ids), ids.into()).unwrap();

        let (_, page) = rows(&app, TOKEN, "section=upNext&offset=2&limit=100").await;
        assert_eq!(page["shuffled"], true);
        let expected: Vec<(String, Value)> = (2..core.up_next_len())
            .map(|i| {
                let row = core.up_next_row(i).unwrap();
                (row.item.to_string(), json!(row.track.unwrap().to_string()))
            })
            .collect();
        assert!(expected.len() > 10);
        assert_eq!(pairs(&page["rows"]), expected);
        let current = core.current_row().unwrap();
        assert_eq!(page["current"]["item"], json!(current.item.to_string()));
        assert_eq!(page["current"]["trackId"], json!(current.track.unwrap().to_string()));
        let (_, history) = rows(&app, TOKEN, "section=history").await;
        let played = core.history_row(0).unwrap();
        assert_eq!(history["rows"][0]["item"], json!(played.row.item.to_string()));
        assert_eq!(history["rows"][0]["play"], json!(played.play));
    }
}

#[tokio::test]
async fn rows_take_at_most_a_hundred() {
    let (_temp, _state, app) = setup().await;
    let tracks = vec!["1"; 150];
    start(&app, TOKEN, start_body(QUEUE, KEY, &tracks, 0)).await;

    let (status, page) = rows(&app, TOKEN, "section=upNext&limit=100").await;
    assert_eq!(
        (status, page["rows"].as_array().unwrap().len(), page["total"].clone()),
        (StatusCode::OK, 100, json!(149))
    );
    let (_, default) = rows(&app, TOKEN, "section=upNext").await;
    assert_eq!(default["rows"].as_array().unwrap().len(), 100);
    for bad in ["limit=101", "limit=0", "limit=-1", "limit=x", "offset=-1", "section=other"] {
        let query = if bad.starts_with("section") {
            bad.to_owned()
        } else {
            format!("section=upNext&{bad}")
        };
        let (status, problem) = rows(&app, TOKEN, &query).await;
        assert_eq!(
            (status, &problem["code"]),
            (StatusCode::UNPROCESSABLE_ENTITY, &json!("validation_failed")),
            "{bad}"
        );
    }
    assert_eq!(rows(&app, TOKEN, "limit=5").await.0, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn rows_are_per_account() {
    let (_temp, state, app) = setup().await;
    assert_eq!(rows(&app, "user2", "section=upNext").await.0, StatusCode::NOT_FOUND);
    start(&app, TOKEN, start_body(QUEUE, KEY, &["1", "1", "1"], 0)).await;
    assert_eq!(rows(&app, "user2", "section=upNext").await.0, StatusCode::NOT_FOUND);

    start(&app, "user2", start_body(OTHER, KEY2, &["2", "2"], 0)).await;
    state.queues.save(&state.db).await;
    let (_, theirs) = rows(&app, "user2", "section=upNext").await;
    assert_eq!(theirs["queueId"], OTHER);
    assert_eq!(pairs(&theirs["rows"]), [(format!("{KEY2}.1"), json!("2"))]);
    let (_, mine) = rows(&app, TOKEN, "section=upNext").await;
    assert_eq!(mine["queueId"], QUEUE);
    assert_eq!(pairs(&mine["rows"]).len(), 2);
}
