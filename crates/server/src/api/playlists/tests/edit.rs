//! `editPlaylist` and `Idempotency-Key`.

use axum::body::Body;
use axum::http::Request;
use jewelcase_core::playlist::{Op, apply_ops};

use super::super::super::testing::respond;
use super::*;
use crate::db::testing::Track;

/// The items of playlist `id` as track IDs in order, through `listPlaylistItems`.
async fn order(app: &Router, id: &str) -> Vec<i64> {
    let (status, page) =
        call(app, TOKEN, "GET", &format!("{URI}/{id}/items?limit=1000"), None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    listed(&page).iter().map(|(track, _)| track.parse().unwrap()).collect()
}

/// `body` sent as `method` on `uri` as `token`, with `key` as its `Idempotency-Key`.
pub(super) async fn send(
    app: &Router,
    token: &str,
    method: &str,
    uri: &str,
    key: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    if let Some(key) = key {
        request = request.header("idempotency-key", key);
    }
    let (status, _, body) =
        respond(app.clone(), request.body(Body::from(body.to_string())).unwrap()).await;
    let body = if body.is_empty() { Value::Null } else { serde_json::from_slice(&body).unwrap() };
    (status, body)
}

/// Edits playlist `id` as the owner.
pub(super) async fn edit(app: &Router, id: &str, body: Value) -> (StatusCode, Value) {
    send(app, TOKEN, "PATCH", &format!("{URI}/{id}"), None, body).await
}

/// `{ "type": "track", "id": ... }`.
pub(super) fn track(id: i64) -> Value {
    json!({ "type": "track", "id": id.to_string() })
}

/// The stored `(key, track)` of every item of playlist `id`, in key order.
async fn rows(db: &Database, id: &str) -> Vec<(i64, i64)> {
    let id: i64 = id.parse().unwrap();
    db.read(move |conn| {
        conn.prepare(
            "SELECT key, track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY key",
        )?
        .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
    })
    .await
    .unwrap()
}

/// How many idempotency keys are stored.
async fn remembered(db: &Database) -> i64 {
    db.read(|conn| conn.query_row("SELECT count(*) FROM idempotency_keys", [], |r| r.get(0)))
        .await
        .unwrap()
}

/// What each track lasts: a second, but track 12's three.
fn lasts(track: i64) -> i64 {
    if track == 12 { 3_000_000 } else { 1_000_000 }
}

/// Applies `ops` to the model and to playlist `id`, and checks the playlist is the model's, in
/// its items, keys, counts and version, which is `*version + 1`.
async fn step(
    (app, db): (&Router, &Database),
    id: &str,
    model: &mut Vec<i64>,
    version: &mut i64,
    ops: Value,
    core: Vec<Op<i64>>,
) -> Value {
    *model = apply_ops(model.clone(), core).unwrap();
    let (status, edited) = edit(app, id, json!({ "baseVersion": *version, "ops": ops })).await;
    assert_eq!(status, StatusCode::OK, "{ops} {edited}");
    *version += 1;
    assert_eq!(edited["version"], *version, "{ops}");
    assert_eq!(edited["trackCount"], model.len(), "{ops}");
    assert_eq!(edited["durationUs"], model.iter().map(|&t| lasts(t)).sum::<i64>(), "{ops}");
    assert_eq!(order(app, id).await, *model, "{ops}");
    let keys: Vec<i64> = rows(db, id).await.iter().map(|row| row.0).collect();
    assert!(keys.windows(2).all(|pair| pair[0] < pair[1]), "{ops} {keys:?}");
    let (_, got) = call(app, TOKEN, "GET", &format!("{URI}/{id}"), None).await;
    assert_eq!(got, edited, "{ops}");
    edited
}

#[tokio::test]
async fn each_op_leaves_the_order_the_shared_core_gives() {
    let (_temp, db, app) = catalog().await;
    db.write(|tx| tx.execute("UPDATE tracks SET duration_us = 3000000 WHERE id = 12", []))
        .await
        .unwrap();
    let made = create(
        &app,
        json!({ "title": "P", "items": [{ "type": "album", "id": "102" }, track(1)] }),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    let mut model = vec![13, 12, 14, 11, 1];
    let mut version = 1;
    let on = (&app, &*db);
    macro_rules! go {
        ($ops:expr, $core:expr) => {
            step(on, id, &mut model, &mut version, $ops, $core).await
        };
    }

    // Adds: at the front, appended, an album in its order, and at the end by position.
    go!(
        json!([{ "op": "add", "items": [track(2)], "at": 0 }]),
        vec![Op::Add { at: Some(0), items: vec![2] }]
    );
    go!(
        json!([{ "op": "add", "items": [{ "type": "album", "id": "102" }] }]),
        vec![Op::Add { at: None, items: vec![13, 12, 14, 11] }]
    );
    let end = model.len();
    go!(
        json!([{ "op": "add", "items": [track(1), track(2)], "at": end }]),
        vec![Op::Add { at: Some(end), items: vec![1, 2] }]
    );
    // Removes, and moves of one, of a block, of a selection that is not contiguous, and to the
    // end.
    go!(
        json!([{ "op": "remove", "positions": [9, 0, 3] }]),
        vec![Op::Remove { positions: vec![9, 0, 3] }]
    );
    go!(
        json!([{ "op": "move", "positions": [1], "at": 0 }]),
        vec![Op::Move { positions: vec![1], at: 0 }]
    );
    go!(
        json!([{ "op": "move", "positions": [3, 4], "at": 0 }]),
        vec![Op::Move { positions: vec![3, 4], at: 0 }]
    );
    go!(
        json!([{ "op": "move", "positions": [0, 4, 6], "at": 2 }]),
        vec![Op::Move { positions: vec![0, 4, 6], at: 2 }]
    );
    let last = model.len() - 1;
    go!(
        json!([{ "op": "move", "positions": [0], "at": last }]),
        vec![Op::Move { positions: vec![0], at: last }]
    );
    // Several ops in one edit, each on what the one before left.
    go!(
        json!([
            { "op": "add", "items": [track(2)], "at": 1 },
            { "op": "remove", "positions": [0] },
            { "op": "move", "positions": [2, 3], "at": 0 },
        ]),
        vec![
            Op::Add { at: Some(1), items: vec![2] },
            Op::Remove { positions: vec![0] },
            Op::Move { positions: vec![2, 3], at: 0 },
        ]
    );
    // The playlist added to itself is what it holds when the edit starts.
    let own = model.clone();
    go!(
        json!([{ "op": "add", "items": [{ "type": "playlist", "id": id }] }]),
        vec![Op::Add { at: None, items: own }]
    );
    // An add that expands to nothing, an album with no tracks, changes the items only by the
    // version.
    db.write(|tx| crate::db::testing::Album::new(103, 1).insert(tx)).await.unwrap();
    go!(
        json!([{ "op": "add", "items": [{ "type": "album", "id": "103" }] }]),
        vec![Op::Add { at: None, items: vec![] }]
    );
}

#[tokio::test]
async fn an_item_between_two_takes_the_midpoint_and_the_rest_keep_their_keys() {
    let (_temp, db, app) = catalog().await;
    let made =
        create(&app, json!({ "title": "P", "items": [track(2), track(13), track(12), track(14)] }))
            .await;
    let id = made["id"].as_str().unwrap();
    let (status, _) = edit(
        &app,
        id,
        json!({ "baseVersion": 1, "ops": [{ "op": "move", "positions": [3], "at": 1 }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let gap = KEY_GAP;
    assert_eq!(rows(&db, id).await, [(gap, 2), (gap + gap / 2, 14), (2 * gap, 13), (3 * gap, 12)]);
    // An add at the front takes a key below the first, and at the end above the last.
    let (status, _) = edit(
        &app,
        id,
        json!({ "baseVersion": 2, "ops": [
            { "op": "add", "items": [track(1)], "at": 0 },
            { "op": "add", "items": [track(11)] },
        ] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        rows(&db, id).await,
        [(0, 1), (gap, 2), (gap + gap / 2, 14), (2 * gap, 13), (3 * gap, 12), (4 * gap, 11)]
    );
}

#[tokio::test]
async fn details_change_alone_or_with_ops_and_the_version_moves_with_them() {
    let (_temp, db, app) = catalog().await;
    let made =
        create(&app, json!({ "title": "Old", "description": "Words", "items": [track(1)] })).await;
    let id = made["id"].as_str().unwrap();
    let before = made["modifiedAt"].as_str().unwrap().to_owned();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let (status, edited) = edit(
        &app,
        id,
        json!({ "baseVersion": 1, "title": "  New  ", "description": null, "pinned": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(
        (&edited["title"], &edited["description"], &edited["pinned"], &edited["version"]),
        (&json!("New"), &Value::Null, &json!(true), &json!(2))
    );
    assert!(edited["modifiedAt"].as_str().unwrap() > before.as_str());
    assert_eq!(edited["createdAt"], made["createdAt"]);
    assert_eq!(tracks(&db, &made["id"]).await, [1]);
    // The name filter and sort see the new title.
    let (_, page) = call(&app, TOKEN, "GET", &format!("{URI}?q=new"), None).await;
    assert_eq!(page["total"], 1);
    let (_, page) = call(&app, TOKEN, "GET", &format!("{URI}?q=old"), None).await;
    assert_eq!(page["total"], 0);

    // A description left out stays; one sent is set; the details and ops go together.
    let (status, edited) = edit(
        &app,
        id,
        json!({ "baseVersion": 2, "description": "Back", "ops": [{ "op": "remove", "positions": [0] }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    let (status, edited) = edit(&app, id, json!({ "baseVersion": 3, "pinned": false })).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(
        (&edited["description"], &edited["pinned"], &edited["trackCount"], &edited["version"]),
        (&json!("Back"), &json!(false), &json!(0), &json!(4))
    );
}

#[tokio::test]
async fn an_edit_that_is_stale_applies_nothing() {
    let (_temp, db, app) = catalog().await;
    let made = create(&app, json!({ "title": "P", "items": [track(1), track(2)] })).await;
    let id = made["id"].as_str().unwrap();
    let (status, _) =
        edit(&app, id, json!({ "baseVersion": 1, "ops": [{ "op": "remove", "positions": [0] }] }))
            .await;
    assert_eq!(status, StatusCode::OK);

    for base in [1, 0, 3, -1] {
        let (status, problem) = edit(
            &app,
            id,
            json!({ "baseVersion": base, "title": "Stale", "ops": [{ "op": "remove", "positions": [0] }] }),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{base}");
        assert_eq!(
            (&problem["code"], &problem["currentVersion"]),
            (&json!("version_conflict"), &json!(2))
        );
    }
    let (_, got) = call(&app, TOKEN, "GET", &format!("{URI}/{id}"), None).await;
    assert_eq!(
        (&got["title"], &got["version"], &got["trackCount"]),
        (&json!("P"), &json!(2), &json!(1))
    );
    assert_eq!(tracks(&db, &made["id"]).await, [2]);

    // Two edits made on the same version: one applies and the other is a conflict.
    let uri = format!("{URI}/{id}");
    let one = send(&app, TOKEN, "PATCH", &uri, None, json!({ "baseVersion": 2, "title": "One" }));
    let two = send(&app, TOKEN, "PATCH", &uri, None, json!({ "baseVersion": 2, "title": "Two" }));
    let (one, two) = tokio::join!(one, two);
    let mut statuses = [one.0, two.0];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::CONFLICT]);
    let (_, got) = call(&app, TOKEN, "GET", &uri, None).await;
    assert_eq!(got["version"], 3);
}

#[tokio::test]
async fn an_edit_that_cannot_apply_applies_nothing() {
    let (_temp, db, app) = catalog().await;
    let made =
        create(&app, json!({ "title": "P", "items": [track(1), track(2), track(13)] })).await;
    let id = made["id"].as_str().unwrap();
    let remove = |positions: Value| json!({ "op": "remove", "positions": positions });
    let cases = [
        json!({}),
        json!({ "baseVersion": 1 }),
        json!({ "baseVersion": "1", "title": "X" }),
        json!({ "baseVersion": 1, "ops": [] }),
        json!({ "baseVersion": 1, "title": "  " }),
        json!({ "baseVersion": 1, "ops": [{ "op": "add", "items": [] }] }),
        json!({ "baseVersion": 1, "ops": [remove(json!([]))] }),
        json!({ "baseVersion": 1, "ops": [{ "op": "move", "positions": [], "at": 0 }] }),
        json!({ "baseVersion": 1, "ops": [{ "op": "move", "positions": [0] }] }),
        json!({ "baseVersion": 1, "ops": [{ "op": "swap", "positions": [0] }] }),
        json!({ "baseVersion": 1, "ops": [remove(json!([-1]))] }),
        json!({ "baseVersion": 1, "ops": [remove(json!([1, 1]))] }),
        // Out of range: a position, a destination, an add's place.
        json!({ "baseVersion": 1, "ops": [remove(json!([3]))] }),
        json!({ "baseVersion": 1, "ops": [{ "op": "move", "positions": [0], "at": 3 }] }),
        json!({ "baseVersion": 1, "ops": [{ "op": "add", "items": [track(2)], "at": 4 }] }),
        // Something not the caller's to add, or not there.
        json!({ "baseVersion": 1, "ops": [{ "op": "add", "items": [track(3)] }] }),
        json!({ "baseVersion": 1, "ops": [{ "op": "add", "items": [track(999)] }] }),
        json!({ "baseVersion": 1, "ops": [{ "op": "add", "items": [{ "type": "external" }] }] }),
        // A good op and a detail change before a bad op are not applied either.
        json!({ "baseVersion": 1, "title": "Changed", "pinned": true, "ops": [
            { "op": "add", "items": [track(2)] }, remove(json!([0, 4])),
        ] }),
        json!({ "baseVersion": 1, "ops": [
            { "op": "add", "items": [track(2)] }, { "op": "add", "items": [track(999)] },
        ] }),
    ];
    for body in cases {
        let (status, problem) = edit(&app, id, body.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body} {problem}");
        assert_eq!(problem["code"], "validation_failed", "{body}");
    }
    let (_, got) = call(&app, TOKEN, "GET", &format!("{URI}/{id}"), None).await;
    assert_eq!(got, made);
    assert_eq!(tracks(&db, &made["id"]).await, [1, 2, 13]);
    assert_eq!(remembered(&db).await, 0);
}

/// Another account, an admin who reaches the library, another library's path and bad IDs find
/// the playlist as absent as for a read, and an item that names someone else's is `422`.
#[tokio::test]
async fn only_the_owner_can_edit_a_playlist() {
    let (temp, db, app) = catalog().await;
    add_others(&temp);
    let made = create(&app, json!({ "title": "Mine", "items": [track(1)] })).await;
    let id = made["id"].as_str().unwrap();
    let body = json!({ "baseVersion": 1, "title": "Taken", "ops": [{ "op": "remove", "positions": [0] }] });
    for (token, uri) in [
        ("user-token", format!("{URI}/{id}")),
        ("admin-token", format!("{URI}/{id}")),
        (TOKEN, format!("{LIBRARY_2}/{id}")),
        ("admin-token", format!("{LIBRARY_2}/{id}")),
        (TOKEN, format!("{URI}/{id}0")),
        (TOKEN, format!("{URI}/nope")),
    ] {
        let (status, problem) = send(&app, token, "PATCH", &uri, None, body.clone()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{token} {uri}");
        assert_eq!(problem["code"], "not_found", "{token} {uri}");
        // Not even a stale version tells them the playlist is there.
        let stale = json!({ "baseVersion": 9, "title": "Taken" });
        let (status, _) = send(&app, token, "PATCH", &uri, None, stale).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{token} {uri}");
    }
    let (_, got) = call(&app, TOKEN, "GET", &format!("{URI}/{id}"), None).await;
    assert_eq!(got, made);

    // Their own playlist is theirs, and the owner's is not theirs to copy from.
    let theirs = create_as(&app, "user-token", json!({ "title": "Theirs" })).await;
    let uri = format!("{URI}/{}", theirs["id"].as_str().unwrap());
    let steal = json!({ "baseVersion": 1, "ops": [{ "op": "add", "items": [{ "type": "playlist", "id": id }] }] });
    let (status, _) = send(&app, "user-token", "PATCH", &uri, None, steal).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) =
        send(&app, TOKEN, "PATCH", &uri, None, json!({ "baseVersion": 1, "title": "X" })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(tracks(&db, &theirs["id"]).await, Vec::<i64>::new());
}

#[tokio::test]
async fn a_repeated_idempotency_key_creates_once_and_is_each_users_own() {
    let (temp, db, app) = catalog().await;
    add_others(&temp);
    let body = json!({ "title": "Once", "items": [track(1), track(2)] });
    let (status, first) = send(&app, TOKEN, "POST", URI, Some("k1"), body.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    // Changed since: the retry still gets what the first request got.
    let id = first["id"].as_str().unwrap();
    let (status, _) = edit(&app, id, json!({ "baseVersion": 1, "title": "Later" })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, again) = send(&app, TOKEN, "POST", URI, Some("k1"), body.clone()).await;
    assert_eq!((status, &again), (StatusCode::CREATED, &first));
    assert_eq!(stored(&db).await, 1);
    assert_eq!(remembered(&db).await, 1);

    // Another user's identical key is not the same key, and another key is another request.
    let (status, theirs) = send(&app, "user-token", "POST", URI, Some("k1"), body.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_ne!(theirs["id"], first["id"]);
    let (status, other) = send(&app, TOKEN, "POST", URI, Some("k2"), body.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_ne!(other["id"], first["id"]);
    assert_eq!(stored(&db).await, 3);
    // Without a key every request creates one.
    send(&app, TOKEN, "POST", URI, None, body.clone()).await;
    send(&app, TOKEN, "POST", URI, None, body.clone()).await;
    assert_eq!(stored(&db).await, 5);

    // Two at once are one playlist.
    let one = send(&app, TOKEN, "POST", URI, Some("race"), body.clone());
    let two = send(&app, TOKEN, "POST", URI, Some("race"), body.clone());
    let (one, two) = tokio::join!(one, two);
    assert_eq!((one.0, two.0), (StatusCode::CREATED, StatusCode::CREATED));
    assert_eq!(one.1, two.1);
    assert_eq!(stored(&db).await, 6);

    // A request that was refused stores nothing, so the key can be used once it is right.
    let before = remembered(&db).await;
    let (status, _) = send(&app, TOKEN, "POST", URI, Some("fix"), json!({ "title": " " })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let bad = json!({ "title": "X", "items": [track(999)] });
    let (status, _) = send(&app, TOKEN, "POST", URI, Some("fix"), bad).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(remembered(&db).await, before);
    let (status, _) = send(&app, TOKEN, "POST", URI, Some("fix"), body).await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn a_repeated_idempotency_key_edits_once_and_is_each_users_own() {
    let (temp, db, app) = catalog().await;
    add_others(&temp);
    let mine = create(&app, json!({ "title": "Mine", "items": [track(1)] })).await;
    let theirs =
        create_as(&app, "user-token", json!({ "title": "Theirs", "items": [track(1)] })).await;
    let (mine, theirs) = (mine["id"].as_str().unwrap(), theirs["id"].as_str().unwrap());
    let add = json!({ "baseVersion": 1, "ops": [{ "op": "add", "items": [track(2)] }] });
    let uri = format!("{URI}/{mine}");

    let (status, first) = send(&app, TOKEN, "PATCH", &uri, Some("e1"), add.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    // The retry names the version the first did, which is stale now, and still gets the answer.
    let (status, again) = send(&app, TOKEN, "PATCH", &uri, Some("e1"), add.clone()).await;
    assert_eq!((status, &again), (StatusCode::OK, &first));
    assert_eq!(first["version"], 2);
    assert_eq!(order(&app, mine).await, [1, 2]);
    // Without the key it is the conflict it is.
    let (status, _) = send(&app, TOKEN, "PATCH", &uri, None, add.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // The same key on another user's playlist applies there.
    let (status, other) =
        send(&app, "user-token", "PATCH", &format!("{URI}/{theirs}"), Some("e1"), add.clone())
            .await;
    assert_eq!(status, StatusCode::OK, "{other}");
    assert_eq!((&other["id"], &other["trackCount"]), (&json!(theirs), &json!(2)));
    assert_eq!(tracks(&db, &json!(theirs)).await, [1, 2]);
    assert_eq!(remembered(&db).await, 2);

    // A refused edit stores nothing: a conflict, then the same key with the version it is at.
    let before = remembered(&db).await;
    let stale = json!({ "baseVersion": 1, "title": "T" });
    let (status, _) = send(&app, TOKEN, "PATCH", &uri, Some("e2"), stale).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(remembered(&db).await, before);
    let (status, fixed) =
        send(&app, TOKEN, "PATCH", &uri, Some("e2"), json!({ "baseVersion": 2, "title": "T" }))
            .await;
    assert_eq!((status, &fixed["version"]), (StatusCode::OK, &json!(3)));

    // Two at once apply once.
    let race = json!({ "baseVersion": 3, "ops": [{ "op": "remove", "positions": [0] }] });
    let one = send(&app, TOKEN, "PATCH", &uri, Some("race"), race.clone());
    let two = send(&app, TOKEN, "PATCH", &uri, Some("race"), race);
    let (one, two) = tokio::join!(one, two);
    assert_eq!((one.0, two.0), (StatusCode::OK, StatusCode::OK));
    assert_eq!(one.1, two.1);
    assert_eq!((&one.1["version"], &one.1["trackCount"]), (&json!(4), &json!(1)));
}

#[tokio::test]
async fn an_idempotency_key_must_be_plain_text_of_at_most_128_characters() {
    let (_temp, db, app) = catalog().await;
    let body = json!({ "title": "K" });
    for key in ["a".repeat(129), String::new()] {
        let (status, problem) = send(&app, TOKEN, "POST", URI, Some(&key), body.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{}", key.len());
        assert_eq!(problem["code"], "validation_failed");
    }
    let made = create(&app, body.clone()).await;
    let uri = format!("{URI}/{}", made["id"].as_str().unwrap());
    let (status, _) = send(
        &app,
        TOKEN,
        "PATCH",
        &uri,
        Some(&"a".repeat(129)),
        json!({ "baseVersion": 1, "title": "L" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(stored(&db).await, 1);
    let (status, _) = send(&app, TOKEN, "POST", URI, Some(&"a".repeat(128)), body).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = send(
        &app,
        TOKEN,
        "PATCH",
        &uri,
        Some(&"b".repeat(128)),
        json!({ "baseVersion": 1, "title": "L" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(remembered(&db).await, 2);
}

/// Seeds playlist 900 of the owner with `n` items, tracks 1000 onward, keys [`KEY_GAP`] apart.
async fn seed(db: &Database, n: i64) {
    db.write(move |tx| {
        for t in 1..=n {
            Track::new(999 + t, 1, 101).insert(tx)?;
        }
        tx.execute(
            "INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, created_at, \
             modified_at, track_count, duration_us) VALUES (900, 1, 1, 'Big', 'big', x'00', 0, 0, ?1, ?2)",
            [n, n * 1_000_000],
        )?;
        tx.execute(
            "INSERT INTO playlist_items SELECT 900, (id - 999) * ?1, 1, id, 0 FROM tracks WHERE id >= 1000",
            [KEY_GAP],
        )
        .map(drop)
    })
    .await
    .unwrap();
}

/// The rows changed since the writer's connection began, counting each insert, update and
/// delete.
async fn changes(db: &Database) -> u64 {
    db.write(|tx| Ok(tx.total_changes())).await.unwrap()
}

#[tokio::test]
async fn an_edit_of_a_large_playlist_writes_only_the_rows_it_changes() {
    let (_temp, db, app) = catalog().await;
    let n = 50_000;
    seed(&db, n).await;
    let uri = format!("{URI}/900");
    let mut model: Vec<i64> = (1000..1000 + n).collect();
    let before = rows(&db, "900").await;
    // The first request of a device writes its last-seen time, which is not what is counted.
    call(&app, TOKEN, "GET", &uri, None).await;

    // Three items moved far apart: each is a delete and an insert, and the playlist one update.
    // Each row written to the account feed is one more write, the playlist's included.
    let ops = json!([{ "op": "move", "positions": [10, 20_000, 49_999], "at": 30_000 }]);
    let start = changes(&db).await;
    let (status, moved) =
        send(&app, TOKEN, "PATCH", &uri, None, json!({ "baseVersion": 1, "ops": ops })).await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(changes(&db).await - start, (3 + 3) * 2 + 2);
    model = apply_ops(model, vec![Op::Move { positions: vec![10, 20_000, 49_999], at: 30_000 }])
        .unwrap();
    let after = rows(&db, "900").await;
    assert_eq!(after.iter().map(|row| row.1).collect::<Vec<_>>(), model);
    let untouched = after.iter().filter(|row| before.contains(row)).count();
    assert_eq!(untouched, usize::try_from(n).unwrap() - 3);
    assert_eq!((&moved["trackCount"], &moved["durationUs"]), (&json!(n), &json!(n * 1_000_000)));

    // Two added in the middle: two inserts; two removed: two deletes.
    let start = changes(&db).await;
    let add = json!({ "baseVersion": 2, "ops": [{ "op": "add", "items": [track(1), track(2)], "at": 100 }] });
    let (status, added) = send(&app, TOKEN, "PATCH", &uri, None, add).await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(changes(&db).await - start, (2 + 1) * 2);
    let start = changes(&db).await;
    let remove = json!({ "baseVersion": 3, "ops": [{ "op": "remove", "positions": [5, 40_000] }] });
    let (status, removed) = send(&app, TOKEN, "PATCH", &uri, None, remove).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(changes(&db).await - start, (2 + 1) * 2);
    assert_eq!(
        (&removed["trackCount"], &removed["durationUs"], &removed["version"]),
        (&json!(n), &json!(n * 1_000_000), &json!(4))
    );
    // A change of details alone writes only the playlist, and its feed row.
    let start = changes(&db).await;
    send(&app, TOKEN, "PATCH", &uri, None, json!({ "baseVersion": 4, "title": "Bigger" })).await;
    assert_eq!(changes(&db).await - start, 2);
}

#[tokio::test]
async fn a_playlist_whose_gap_is_used_up_is_renumbered_in_the_same_edit() {
    let (_temp, db, app) = catalog().await;
    let made = create(
        &app,
        json!({ "title": "P", "items": [track(2), track(13), track(12), track(14), track(11)] }),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    // Keys 1 to 5, added a second apart, so nothing fits between two.
    db.write(|tx| {
        tx.execute(
            "UPDATE playlist_items SET added_at = key / ?1 * 1000, key = key / ?1",
            [KEY_GAP],
        )
    })
    .await
    .unwrap();
    let (status, edited) = edit(
        &app,
        id,
        json!({ "baseVersion": 1, "ops": [{ "op": "move", "positions": [4], "at": 1 }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(order(&app, id).await, [2, 11, 13, 12, 14]);
    let gap = KEY_GAP;
    assert_eq!(
        rows(&db, id).await,
        [(gap, 2), (2 * gap, 11), (3 * gap, 13), (4 * gap, 12), (5 * gap, 14)]
    );
    // Each item kept the time it was added; the moved one, from before.
    let added: Vec<(i64, i64)> = db
        .read(|conn| {
            conn.prepare("SELECT track_id, added_at FROM playlist_items ORDER BY key")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .await
        .unwrap();
    assert_eq!(added, [(2, 1000), (11, 5000), (13, 2000), (12, 3000), (14, 4000)]);
    assert_eq!((&edited["trackCount"], &edited["version"]), (&json!(5), &json!(2)));
    // Then there is room again, and the next move takes the midpoint.
    let (status, _) = edit(
        &app,
        id,
        json!({ "baseVersion": 2, "ops": [{ "op": "move", "positions": [4], "at": 0 }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(order(&app, id).await, [14, 2, 11, 13, 12]);
    assert_eq!(rows(&db, id).await[0], (0, 14));
}

/// Totals are summed from the tracks' durations as they are when the edit is made, so a track
/// that was rescanned to another length is counted at that length.
#[tokio::test]
async fn an_edit_sums_the_durations_the_tracks_have_now() {
    let (_temp, db, app) = catalog().await;
    let made = create(&app, json!({ "title": "P", "items": [track(1), track(2)] })).await;
    assert_eq!(made["durationUs"], 2_000_000);
    db.write(|tx| tx.execute("UPDATE tracks SET duration_us = 5000000 WHERE id = 1", []))
        .await
        .unwrap();
    let id = made["id"].as_str().unwrap();
    let (status, edited) =
        edit(&app, id, json!({ "baseVersion": 1, "ops": [{ "op": "remove", "positions": [1] }] }))
            .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!((&edited["trackCount"], &edited["durationUs"]), (&json!(1), &json!(5_000_000)));
}

/// The ops are applied off the writer, so the write checks again that the playlist is as the
/// plan found it. An edit that came between is a conflict, and a track the plan adds that was
/// purged since sends it back to be made again; either way nothing is written.
#[tokio::test]
async fn a_plan_is_written_only_while_the_playlist_is_as_it_was_planned_from() {
    let (_temp, db, app) = catalog().await;
    let made = create(&app, json!({ "title": "P", "items": [track(1), track(2)] })).await;
    let id: i64 = made["id"].as_str().unwrap().parse().unwrap();
    let plan =
        |ops: Vec<Op<i64>>| db.try_read(move |conn| super::super::edit::plan(conn, 1, id, ops, 5));
    let write = |plan: super::super::edit::Plan, base: i64| {
        db.try_write(move |tx| {
            use super::super::edit::{fresh, write};
            if !fresh(tx, 1, &plan)? {
                return Ok(false);
            }
            write(tx, 1, 1, id, base, &plan).map(|()| true)
        })
    };

    let remove = plan(vec![Op::Remove { positions: vec![0] }]).await.unwrap();
    let (status, _) = edit(&app, &id.to_string(), json!({ "baseVersion": 1, "title": "Q" })).await;
    assert_eq!(status, StatusCode::OK);
    let problem = serde_json::to_value(write(remove, 1).await.unwrap_err()).unwrap();
    assert_eq!(
        (&problem["code"], &problem["currentVersion"]),
        (&json!("version_conflict"), &json!(2))
    );
    assert_eq!(tracks(&db, &made["id"]).await, [1, 2]);

    let add = plan(vec![Op::Add { at: Some(0), items: vec![14, 1] }]).await.unwrap();
    db.write(|tx| tx.execute("DELETE FROM tracks WHERE id = 14", [])).await.unwrap();
    assert!(!write(add, 2).await.unwrap(), "a plan whose added track is gone is made again");
    assert_eq!(tracks(&db, &made["id"]).await, [1, 2]);
    let add = plan(vec![Op::Add { at: Some(0), items: vec![14, 1] }]).await.unwrap();
    assert_eq!(add.totals, Some((3, 3_000_000)));
    assert!(write(add, 2).await.unwrap());
    assert_eq!(tracks(&db, &made["id"]).await, [1, 1, 2]);
}

#[tokio::test]
async fn an_announced_add_leaves_out_tracks_purged_since_the_expansion() {
    let (_temp, db, app) = catalog().await;
    let playlist = create(&app, json!({ "title": "Mix", "items": [track(2)] })).await;
    let id: i64 = playlist["id"].as_str().unwrap().parse().unwrap();
    let ops = vec![Op::Add { at: None, items: vec![1, 999_999] }];
    let plan =
        db.try_read(move |conn| super::super::edit::plan(conn, 1, id, ops, 5)).await.unwrap();
    assert_eq!(plan.totals.map(|(count, _)| count), Some(2), "only track 1 was added");
    let [super::super::announce::AppliedOp::Add { track_ids, .. }] = &plan.applied[..] else {
        panic!("one add expected");
    };
    assert_eq!(serde_json::to_value(track_ids).unwrap(), json!(["1"]));
    db.try_write(move |tx| super::super::edit::write(tx, 1, 1, id, 1, &plan)).await.unwrap();
    assert_eq!(tracks(&db, &playlist["id"]).await, [2, 1]);
}

/// A free `id` is the playlist's, and a taken one is `409 id_taken` whoever's it is, and changes
/// nothing; one that is not a positive 63-bit integer in decimal is `422`.
#[tokio::test]
async fn a_playlist_is_created_under_the_id_the_client_chose() {
    let (temp, db, app) = catalog().await;
    add_user(&conn(&temp), 2, "user");
    add_device(&conn(&temp), 2, 3, "user2");
    access(&conn(&temp), 2, 1).unwrap();
    let chosen = "4611686018427387904";

    let (status, made) =
        send(&app, TOKEN, "POST", URI, None, json!({ "title": "A", "id": chosen })).await;
    assert_eq!((status, &made["id"]), (StatusCode::CREATED, &json!(chosen)));
    let (status, got) = call(&app, TOKEN, "GET", &format!("{URI}/{chosen}"), None).await;
    assert_eq!((status, got["title"].clone()), (StatusCode::OK, json!("A")));

    for token in [TOKEN, "user2"] {
        let (status, problem) =
            send(&app, token, "POST", URI, None, json!({ "title": "B", "id": chosen })).await;
        assert_eq!(
            (status, &problem["code"]),
            (StatusCode::CONFLICT, &json!("id_taken")),
            "{token}"
        );
    }
    let count = |db: Arc<Database>| async move {
        db.read(|conn| conn.query_row("SELECT count(*) FROM playlists", [], |r| r.get::<_, i64>(0)))
            .await
            .unwrap()
    };
    assert_eq!(count(db.clone()).await, 1);

    for id in
        [json!("0"), json!("-1"), json!("01"), json!("abc"), json!("9223372036854775808"), json!(5)]
    {
        let (status, _) =
            send(&app, TOKEN, "POST", URI, None, json!({ "title": "C", "id": id })).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{id}");
    }
    assert_eq!(count(db).await, 1);
}

/// A create repeated under its key after it applied answers as the first did, though its `id` is
/// taken by then: it is the same request, not a second one.
#[tokio::test]
async fn a_create_with_an_id_repeated_under_its_key_is_answered_not_refused() {
    let (_temp, _db, app) = catalog().await;
    let body = json!({ "title": "A", "id": "77" });
    let first = send(&app, TOKEN, "POST", URI, Some("create-77"), body.clone()).await;
    assert_eq!(first.0, StatusCode::CREATED);
    assert_eq!(send(&app, TOKEN, "POST", URI, Some("create-77"), body.clone()).await, first);
    let (status, problem) = send(&app, TOKEN, "POST", URI, Some("another"), body).await;
    assert_eq!((status, &problem["code"]), (StatusCode::CONFLICT, &json!("id_taken")));
}
