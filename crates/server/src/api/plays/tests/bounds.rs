//! How much one `recordPlays` request may hold: at most 500 reports, each with a `context` and
//! `reason` of bounded size, so no request holds the single writer for long. And that a report's
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
async fn an_over_long_context_or_reason_is_refused() {
    let (_temp, db, app) = app_with_tracks().await;
    let mut long_id = report("1", 0, Value::Null);
    long_id["context"]["id"] = json!("1".repeat(101));
    let mut long_text = report("1", 0, Value::Null);
    long_text["reason"] = json!({ "kind": "mood", "text": "t".repeat(501) });
    for bad in [long_id, long_text] {
        let status = report_as_owner(&app, json!([bad])).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
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
