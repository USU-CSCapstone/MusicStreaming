use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};

use super::super::testing::{add_device, add_user, app_with_tracks, owner_request, respond};
use crate::db::Database;

const PLAY: &str = "5f0c6a52-3b7e-4d43-9a8e-0d1c2b3a4f5e";

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

async fn post(app: &Router, request: axum::http::request::Builder, items: Value) -> StatusCode {
    let body = Body::from(json!({ "items": items }).to_string());
    let request = request.header(header::CONTENT_TYPE, "application/json").body(body).unwrap();
    respond(app.clone(), request).await.0
}

async fn report_as_owner(app: &Router, items: Value) -> StatusCode {
    post(app, owner_request("POST", "/api/v1/me/plays"), items).await
}

async fn plays(db: &Database) -> Vec<(String, i64, i64, i64, Option<String>)> {
    db.read(|conn| {
        conn.prepare(
            "SELECT id, user_id, started_at, listen_time_ms, ended FROM plays ORDER BY id",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?
        .collect()
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

    let reports = json!([report("1", 30_000, json!("skipped")), report("1", 10_000, Value::Null)]);
    assert_eq!(report_as_owner(&app, reports).await, StatusCode::NO_CONTENT);
    assert_eq!(
        plays(&db).await,
        [(PLAY.into(), 1, started, 30_000, Some("skipped".into()))],
        "an older report arriving late takes nothing back"
    );
    let reports = json!([report("1", 31_000, json!("skipped"))]);
    assert_eq!(report_as_owner(&app, reports).await, StatusCode::NO_CONTENT);
    let ends: i64 = db
        .read(|conn| conn.query_row("SELECT count(*) FROM play_ends", [], |row| row.get(0)))
        .await
        .unwrap();
    assert_eq!(ends, 1, "it ended once, for the played hook, however often it is reported");
}

#[tokio::test]
async fn a_play_out_of_reach_is_left_out_and_someone_elses_is_never_changed() {
    let (temp, db, app) = app_with_tracks().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    conn.execute("INSERT INTO library_access VALUES (2, 1, 0)", []).unwrap();
    let as_user = || {
        Request::builder()
            .method("POST")
            .uri("/api/v1/me/plays")
            .header(header::AUTHORIZATION, "Bearer user-token")
    };

    let mut elsewhere = report("3", 0, Value::Null);
    elsewhere["libraryId"] = json!("2");
    assert_eq!(post(&app, as_user(), json!([elsewhere])).await, StatusCode::NO_CONTENT);
    let mut not_there = report("3", 0, Value::Null);
    not_there["libraryId"] = json!("1");
    assert_eq!(report_as_owner(&app, json!([not_there])).await, StatusCode::NO_CONTENT);
    assert!(
        plays(&db).await.is_empty(),
        "neither a library they cannot reach nor a track not in it"
    );

    assert_eq!(
        report_as_owner(&app, json!([report("1", 5_000, Value::Null)])).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post(&app, as_user(), json!([report("1", 99_000, json!("finished"))])).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(plays(&db).await[0].3, 5_000, "the owner's play is theirs alone");
}

#[tokio::test]
async fn a_malformed_report_records_nothing() {
    let (_temp, db, app) = app_with_tracks().await;
    let mut other = report("2", 0, Value::Null);
    other["playId"] = json!("0e3f4b1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b");
    for (field, value) in [
        ("playId", json!("not-a-uuid")),
        ("startedAt", json!("yesterday")),
        ("trackId", Value::Null),
        ("context", json!({})),
        ("origin", json!("radio")),
        ("end", json!("paused")),
        ("listenTimeMs", json!(-1)),
    ] {
        let mut bad = report("1", 0, Value::Null);
        bad[field] = value;
        let status = report_as_owner(&app, json!([other, bad])).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{field}");
    }
    assert!(plays(&db).await.is_empty());
}
