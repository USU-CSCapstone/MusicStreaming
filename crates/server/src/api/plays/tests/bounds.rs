//! How much one `recordPlays` request may hold: at most 500 reports, each with a `context` and
//! `reason` of at most 4 KiB, so no request holds the single writer for long. And that a report's
//! `deviceId` is not read, as a play is the reporting device's.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::super::testing::{add_device, app_with_tracks, conn};
use super::{plays, post, report, report_as_owner};

/// `n` reports, each of a play of its own.
fn reports(n: u32) -> Value {
    let play = |n| {
        let mut played = report("1", 0, Value::Null);
        played["playId"] = json!(format!("00000000-0000-4000-8000-{n:012}"));
        played
    };
    (0..n).map(play).collect()
}

#[tokio::test]
async fn a_request_holds_at_most_500_reports() {
    let (_temp, db, app) = app_with_tracks().await;
    assert_eq!(report_as_owner(&app, reports(501)).await, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(plays(&db).await.is_empty(), "none of them recorded");
    assert_eq!(report_as_owner(&app, reports(500)).await, StatusCode::NO_CONTENT);
    assert_eq!(plays(&db).await.len(), 500);
}

#[tokio::test]
async fn a_context_or_reason_that_is_oversized_or_the_wrong_shape_is_refused() {
    let (_temp, db, app) = app_with_tracks().await;
    let big = "x".repeat(4096);
    let with = |field: &str, value: Value| {
        let mut played = report("1", 0, Value::Null);
        played[field] = value;
        played
    };
    let bad = [
        with("context", json!({ "type": "album", "id": big })),
        with("context", json!({ "id": "101" })),
        with("context", json!({ "type": 1 })),
        with("context", json!("album")),
        with("reason", json!({ "kind": "mood", "text": big })),
        with("reason", json!("mood")),
    ];
    for bad in bad {
        let status = report_as_owner(&app, json!([bad])).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad:.80}");
    }
    assert!(plays(&db).await.is_empty());
}

/// A `deviceId` in a report is ignored, whatever it names: the play is recorded on the device
/// whose token reports it.
#[tokio::test]
async fn a_reports_device_id_is_not_read() {
    let (temp, db, app) = app_with_tracks().await;
    add_device(&conn(&temp), 1, 2, "phone");
    let mut played = report("1", 0, Value::Null);
    played["deviceId"] = json!("1");
    assert_eq!(post(&app, "phone", json!([played])).await, StatusCode::NO_CONTENT);
    let device: i64 = db
        .read(|conn| conn.query_row("SELECT device_id FROM plays", [], |row| row.get(0)))
        .await
        .unwrap();
    assert_eq!(device, 2);
}
