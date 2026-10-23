use std::net::SocketAddr;

use axum::Router;
use axum::http::StatusCode;
use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::super::testing::player::{progress_report, report, start_playing};
use super::super::testing::socket::{connect_with, next, next_of, serve};
use super::super::testing::{TOKEN, add_device, add_user, app_with_tracks, call, conn};
use crate::db::testing::access;

mod disconnected;
mod saved;

/// [`app_with_tracks`], served, with the owner also on device 2 as `phone`, and user 2, who
/// reaches only library 1, on device 3 as `user2`.
async fn setup() -> (tempfile::TempDir, Router, SocketAddr) {
    let (temp, _db, app) = app_with_tracks().await;
    let conn = conn(&temp);
    add_device(&conn, 1, 2, "phone");
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 3, "user2");
    access(&conn, 2, 1).unwrap();
    let address = serve(app.clone()).await;
    (temp, app, address)
}

/// `getPlayer`, logged in with `token`.
async fn player(app: &Router, token: &str) -> Value {
    let (status, player) = call(app, token, "GET", "/api/v1/me/player", None).await;
    assert_eq!(status, StatusCode::OK);
    player
}

#[tokio::test]
async fn before_anything_plays_the_player_is_stopped() {
    let (_temp, app, _) = setup().await;
    let mut state = player(&app, TOKEN).await;
    assert!(state["positionAt"].as_str().unwrap().ends_with('Z'), "{state}");
    state.as_object_mut().unwrap().remove("positionAt");
    assert_eq!(
        state,
        json!({
            "status": "stopped", "activeDeviceId": null, "itemId": null, "libraryId": null,
            "trackId": null, "positionMs": 0, "volume": null,
        })
    );
}

#[tokio::test]
async fn playback_state_reaches_every_connection_of_the_account() {
    let (_temp, _app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    let mut stranger = connect_with(address, "user2").await;
    for socket in [&mut desk, &mut phone, &mut stranger] {
        let greeting = next_of(socket, "player").await;
        assert_eq!(
            greeting["state"]["status"],
            json!("stopped"),
            "the current state on connecting"
        );
    }

    let changes = json!({ "trackId": "2", "positionMs": 500, "start": true });
    let report = json!({ "type": "progress", "report": progress_report("1", "playing", changes) });
    desk.send(Message::text(report.to_string())).await.unwrap();
    for socket in [&mut phone, &mut desk] {
        let state = &next_of(socket, "player").await["state"];
        assert_eq!((&state["activeDeviceId"], &state["trackId"]), (&json!("1"), &json!("2")));
    }
    // Its own account's device list is the one other message it is sent.
    next_of(&mut stranger, "devices").await;
    stranger.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    assert_eq!(next(&mut stranger).await["type"], json!("pong"), "no other account's state");
}

#[tokio::test]
async fn starting_takes_over_and_only_the_active_device_reports() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    report(&mut desk, "1", "playing", json!({})).await;
    assert_eq!(player(&app, TOKEN).await["status"], json!("stopped"), "none is active");

    report(&mut desk, "1", "playing", json!({ "start": true, "volume": 0.5 })).await;
    let state = player(&app, TOKEN).await;
    assert_eq!(
        (&state["status"], &state["activeDeviceId"], &state["itemId"]),
        (&json!("playing"), &json!("1"), &json!("item-1"))
    );
    assert_eq!((&state["libraryId"], &state["trackId"]), (&json!("1"), &json!("1")));
    assert_eq!((&state["positionMs"], &state["volume"]), (&json!(1000), &json!(0.5)));

    // The phone starts, buffering first; the desk keeps reporting until it hears.
    let start = json!({ "start": true, "trackId": "2", "positionMs": 0 });
    report(&mut phone, "2", "buffering", start).await;
    report(&mut desk, "1", "playing", json!({})).await;
    let state = player(&app, TOKEN).await;
    assert_eq!(
        (&state["status"], &state["activeDeviceId"], &state["trackId"], &state["volume"]),
        (&json!("paused"), &json!("2"), &json!("2"), &json!(null))
    );

    report(&mut phone, "2", "stopped", json!({})).await;
    let state = player(&app, TOKEN).await;
    assert_eq!(
        (&state["status"], &state["activeDeviceId"], &state["positionMs"]),
        (&json!("stopped"), &json!(null), &json!(1000)),
        "where it stopped is kept"
    );
    report(&mut phone, "2", "paused", json!({})).await;
    assert_eq!(player(&app, TOKEN).await["status"], json!("stopped"));
}

#[tokio::test]
async fn a_report_without_volume_keeps_the_playing_devices_volume() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    report(&mut desk, "1", "playing", json!({ "start": true, "volume": 0.5 })).await;
    report(&mut desk, "1", "paused", json!({})).await;
    assert_eq!(player(&app, TOKEN).await["volume"], json!(0.5));
}

#[tokio::test]
async fn reports_are_checked() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut user2 = connect_with(address, "user2").await;
    for changes in [
        json!({ "deviceId": "2" }),
        json!({ "itemId": "" }),
        json!({ "itemId": "x".repeat(101) }),
        json!({ "volume": 1.5 }),
        json!({ "positionMs": -1 }),
        json!({ "status": "rewinding" }),
        // Out of reach or not there, which are the same (`requirements/users.md` §10).
        json!({ "libraryId": "2", "trackId": "1" }),
        json!({ "trackId": "01" }),
    ] {
        let mut changes = changes;
        changes["start"] = json!(true);
        report(&mut desk, "1", "playing", changes.clone()).await;
        assert_eq!(player(&app, TOKEN).await["status"], json!("stopped"), "{changes}");
    }
    let elsewhere = json!({ "start": true, "libraryId": "2", "trackId": "3" });
    report(&mut user2, "3", "playing", elsewhere).await;
    assert_eq!(player(&app, "user2").await["status"], json!("stopped"), "library 2");
    start_playing(&mut user2, "3").await;
    assert_eq!(player(&app, "user2").await["status"], json!("playing"), "library 1");
}

#[tokio::test]
async fn of_a_devices_connections_only_the_one_playing_reports() {
    let (_temp, app, address) = setup().await;
    let mut tab = connect_with(address, TOKEN).await;
    let mut other_tab = connect_with(address, TOKEN).await;
    start_playing(&mut tab, "1").await;
    report(&mut other_tab, "1", "playing", json!({ "start": true, "trackId": "2" })).await;
    report(&mut tab, "1", "playing", json!({})).await;
    assert_eq!(player(&app, TOKEN).await["trackId"], json!("2"), "the tab that started");
}

#[tokio::test]
async fn a_report_is_checked_against_the_account_as_it_is_now() {
    let (temp, app, address) = setup().await;
    let mut admin = connect_with(address, TOKEN).await;
    let elsewhere = json!({ "start": true, "libraryId": "2", "trackId": "3" });
    report(&mut admin, "1", "playing", elsewhere.clone()).await;
    assert_eq!(player(&app, TOKEN).await["trackId"], json!("3"), "an admin reaches every library");

    // Signed out by a path that does not close its connections.
    let mut phone = connect_with(address, "phone").await;
    let conn = conn(&temp);
    let revoke = "UPDATE devices SET revoked_reason = 'credentials_changed' WHERE id = 2";
    conn.execute(revoke, []).unwrap();
    start_playing(&mut phone, "2").await;
    assert_eq!(player(&app, TOKEN).await["activeDeviceId"], json!("1"));

    // Suspended the same way. Its report is refused, and answered with its state.
    let mut user2 = connect_with(address, "user2").await;
    next_of(&mut user2, "player").await;
    conn.execute("UPDATE users SET status = 'suspended' WHERE id = 2", []).unwrap();
    let start = progress_report("3", "playing", json!({ "start": true }));
    let start = json!({ "type": "progress", "report": start });
    user2.send(Message::text(start.to_string())).await.unwrap();
    assert_eq!(next_of(&mut user2, "player").await["state"]["status"], json!("stopped"));
}

#[tokio::test]
async fn a_refused_report_is_answered_with_the_current_state() {
    let (_temp, _app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    next_of(&mut desk, "player").await;
    // Not through `report`, whose wait for a pong would skip the answer if it came first.
    let refused = progress_report("1", "playing", json!({}));
    let refused = json!({ "type": "progress", "report": refused });
    desk.send(Message::text(refused.to_string())).await.unwrap();
    let state = next_of(&mut desk, "player").await;
    assert_eq!(state["state"]["status"], json!("stopped"));
}
