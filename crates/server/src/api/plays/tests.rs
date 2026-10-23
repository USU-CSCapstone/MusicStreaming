use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::testing::{TOKEN, add_device, add_user, app_with_tracks, call, conn};
use crate::db::Database;
use crate::db::testing::access;

mod checks;
mod keys;
mod outlives;
mod snapshot;

const PLAY: &str = "5f0c6a52-3b7e-4d43-9a8e-0d1c2b3a4f5e";

/// A report of play [`PLAY`] of `track` in library 1, `listened` milliseconds in, with `end`.
fn report(track: &str, listened: u32, end: Value) -> Value {
    json!({
        "playId": PLAY,
        "libraryId": "1",
        "trackId": track,
        "startedAt": "2026-10-02T12:00:00.500Z",
        "listenTimeMs": listened,
        "end": end,
        "context": { "type": "album", "id": "101" },
        "origin": "context",
        "deviceId": "1"
    })
}

/// Reports `items` as plays, logged in with `token`, and returns the status.
async fn post(app: &Router, token: &str, items: Value) -> StatusCode {
    let body = Some(json!({ "items": items }));
    call(app, token, "POST", "/api/v1/me/plays", body).await.0
}

/// Reports `items` as plays as the owner, and returns the status.
async fn report_as_owner(app: &Router, items: Value) -> StatusCode {
    post(app, TOKEN, items).await
}

/// Every stored play's ID, user, start, listen time and end, by ID and then user.
async fn plays(db: &Database) -> Vec<(String, i64, i64, i64, Option<String>)> {
    db.read(|conn| {
        conn.prepare(
            "SELECT id, user_id, started_at, listen_time_ms, ended FROM plays ORDER BY id, user_id",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?
        .collect()
    })
    .await
    .unwrap()
}

/// The number of play events of `kind`, `started` or `ended`, for the playing and played hooks.
async fn events(db: &Database, kind: &'static str) -> i64 {
    db.read(move |conn| {
        conn.query_row("SELECT count(*) FROM play_events WHERE kind = ?1", [kind], |row| row.get(0))
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn a_play_is_recorded_once_and_keeps_its_most_listened_and_its_end() {
    let (_temp, db, app) = app_with_tracks().await;
    let started = 1_790_942_400_500; // 2026-10-02T12:00:00.500Z
    assert_eq!(
        report_as_owner(&app, json!([report("1", 0, Value::Null)])).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(plays(&db).await, [(PLAY.into(), 1, started, 0, None)]);

    // In capitals it is the same play.
    let mut louder = report("1", 30_000, json!("skipped"));
    louder["playId"] = json!(PLAY.to_uppercase());
    let reports = json!([louder, report("1", 10_000, Value::Null)]);
    assert_eq!(report_as_owner(&app, reports).await, StatusCode::NO_CONTENT);
    assert_eq!(
        plays(&db).await,
        [(PLAY.into(), 1, started, 30_000, Some("skipped".into()))],
        "an older report arriving late takes nothing back"
    );
    let reports = json!([report("1", 31_000, json!("skipped"))]);
    assert_eq!(report_as_owner(&app, reports).await, StatusCode::NO_CONTENT);
    assert_eq!(
        events(&db, "ended").await,
        1,
        "it ended once, for the played hook, however often it is reported"
    );
}

#[tokio::test]
async fn a_play_first_reported_already_ended_ends_once_and_never_starts() {
    let (_temp, db, app) = app_with_tracks().await;
    let reports = json!([report("1", 30_000, json!("finished"))]);
    assert_eq!(report_as_owner(&app, reports).await, StatusCode::NO_CONTENT);
    assert_eq!(events(&db, "ended").await, 1, "the played hook hears of it");
    assert_eq!(events(&db, "started").await, 0, "it was never playing as far as we knew");
}

/// Adds user 2, who reaches library 1 and is logged in on device 2 as `user-token`.
fn add_second_user(temp: &tempfile::TempDir) {
    let conn = conn(temp);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    access(&conn, 2, 1).unwrap();
}

#[tokio::test]
async fn a_play_out_of_reach_is_left_out() {
    let (temp, db, app) = app_with_tracks().await;
    add_second_user(&temp);
    let mut elsewhere = report("3", 0, Value::Null);
    elsewhere["libraryId"] = json!("2");
    assert_eq!(post(&app, "user-token", json!([elsewhere])).await, StatusCode::NO_CONTENT);
    let mut not_there = report("3", 0, Value::Null);
    not_there["libraryId"] = json!("1");
    assert_eq!(report_as_owner(&app, json!([not_there])).await, StatusCode::NO_CONTENT);
    // An ID that is not one names nothing, so it is left out the same way.
    for field in ["libraryId", "trackId"] {
        let mut malformed = report("1", 0, Value::Null);
        malformed[field] = json!("x");
        let status = report_as_owner(&app, json!([malformed])).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{field}");
    }
    assert!(
        plays(&db).await.is_empty(),
        "neither a library they cannot reach, a track not in it, nor a malformed ID"
    );
}

#[tokio::test]
async fn two_users_plays_under_one_id_are_each_their_own() {
    let (temp, db, app) = app_with_tracks().await;
    add_second_user(&temp);
    let started = 1_790_942_400_500;
    // The users whose plays ended, in the order they did.
    let ended = || {
        db.read(|conn| {
            conn.prepare("SELECT user_id FROM play_events WHERE kind = 'ended' ORDER BY seq")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()
        })
    };

    let owners = report("1", 5_000, Value::Null);
    assert_eq!(report_as_owner(&app, json!([owners])).await, StatusCode::NO_CONTENT);
    let theirs = report("2", 99_000, json!("finished"));
    assert_eq!(post(&app, "user-token", json!([theirs])).await, StatusCode::NO_CONTENT);
    let finished = Some("finished".to_owned());
    assert_eq!(
        plays(&db).await,
        [
            (PLAY.into(), 1, started, 5_000, None),
            (PLAY.into(), 2, started, 99_000, finished.clone())
        ],
        "both are kept, and the second user's report leaves the owner's play as it was"
    );
    assert_eq!(ended().await.unwrap(), [2], "only the play that ended reaches the played hook");

    let owners = report("1", 6_000, json!("skipped"));
    assert_eq!(report_as_owner(&app, json!([owners])).await, StatusCode::NO_CONTENT);
    let skipped = Some("skipped".to_owned());
    assert_eq!(
        plays(&db).await,
        [(PLAY.into(), 1, started, 6_000, skipped), (PLAY.into(), 2, started, 99_000, finished)],
        "the owner's report changes only theirs"
    );
    assert_eq!(ended().await.unwrap(), [2, 1]);
}

#[tokio::test]
async fn a_malformed_report_records_nothing() {
    let (_temp, db, app) = app_with_tracks().await;
    let mut other = report("2", 0, Value::Null);
    other["playId"] = json!("0e3f4b1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b");
    let external = json!({ "type": "external", "sourceId": "s", "externalId": "e" });
    for (field, value) in [
        ("playId", json!("not-a-uuid")),
        ("startedAt", json!("yesterday")),
        ("startedAt", json!("now")),
        ("startedAt", json!("2460000")),
        ("startedAt", json!("2026-10-02")),
        ("startedAt", json!("2026-10-02T12:00:00")),
        ("trackId", Value::Null),
        ("external", external),
        ("context", json!({})),
        ("context", json!({ "type": "album" })),
        ("reason", json!({ "type": "radio", "seed": { "trackId": "1" } })),
        ("origin", json!("radio")),
        ("end", json!("paused")),
        ("listenTimeMs", json!(-1)),
    ] {
        let mut bad = report("1", 0, Value::Null);
        bad[field] = value.clone();
        let status = report_as_owner(&app, json!([other, bad])).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{field}: {value}");
    }
    assert!(plays(&db).await.is_empty());
}

/// A report's `context` and `reason` are stored as compact JSON holding only what the spec
/// defines, and its `origin` by name.
#[tokio::test]
async fn context_origin_and_reason_are_stored_as_the_spec_defines_them() {
    let (_temp, db, app) = app_with_tracks().await;
    let mut played = report("1", 0, Value::Null);
    played["context"]["label"] = json!("Blue");
    played["reason"] = json!({ "kind": "radioSeed", "text": "Radio from Blue", "extra": 1 });
    assert_eq!(report_as_owner(&app, json!([played])).await, StatusCode::NO_CONTENT);
    let stored: (String, String, String) = db
        .read(|conn| {
            let sql = "SELECT context, origin, reason FROM plays";
            conn.query_row(sql, [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        })
        .await
        .unwrap();
    assert_eq!(
        stored,
        (
            r#"{"type":"album","id":"101"}"#.to_owned(),
            "context".to_owned(),
            r#"{"kind":"radioSeed","text":"Radio from Blue"}"#.to_owned()
        )
    );
}
