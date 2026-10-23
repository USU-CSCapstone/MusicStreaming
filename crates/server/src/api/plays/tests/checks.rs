//! What a report's `startedAt` and `external` make of it: a start is read as RFC 3339 and never
//! later than the server's clock, and a play of an external track is left out without failing
//! the rest.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::super::testing::app_with_tracks;
use super::{PLAY, plays, report, report_as_owner};
use crate::db::now_ms;

#[tokio::test]
async fn an_external_play_is_left_out_and_the_rest_recorded() {
    let (_temp, db, app) = app_with_tracks().await;
    let mut external = report("1", 0, Value::Null);
    external.as_object_mut().unwrap().remove("trackId");
    external["playId"] = json!("0e3f4b1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b");
    external["external"] = json!({ "type": "external", "sourceId": "s", "externalId": "e" });
    let reports = json!([external, report("1", 5_000, Value::Null)]);
    assert_eq!(report_as_owner(&app, reports).await, StatusCode::NO_CONTENT);
    let started = 1_790_942_400_500; // 2026-10-02T12:00:00.500Z
    assert_eq!(plays(&db).await, [(PLAY.into(), 1, started, 5_000, None)]);
}

#[tokio::test]
async fn a_start_is_read_with_its_offset() {
    let (_temp, db, app) = app_with_tracks().await;
    let mut played = report("1", 0, Value::Null);
    played["startedAt"] = json!("2026-10-02T14:00:00.500+02:00");
    assert_eq!(report_as_owner(&app, json!([played])).await, StatusCode::NO_CONTENT);
    assert_eq!(plays(&db).await[0].2, 1_790_942_400_500, "2026-10-02T12:00:00.500Z");
}

/// A device's clock can run ahead of the server's, and its plays are still the user's.
#[tokio::test]
async fn a_start_after_the_servers_time_is_taken_as_now() {
    let (_temp, db, app) = app_with_tracks().await;
    let mut played = report("1", 0, Value::Null);
    played["startedAt"] = json!("9999-12-31T23:59:59.999Z");
    let before = now_ms();
    assert_eq!(report_as_owner(&app, json!([played])).await, StatusCode::NO_CONTENT);
    let started = plays(&db).await[0].2;
    assert!((before..=now_ms()).contains(&started), "{started} is when it was heard");
}
