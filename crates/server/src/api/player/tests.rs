use std::net::SocketAddr;

use axum::Router;
use axum::http::StatusCode;
use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::super::testing::player::{
    command_id, context, item, progress_report, report, start_playing, start_queue,
};
use super::super::testing::socket::{connect_with, next, next_of, serve};
use super::super::testing::{TOKEN, add_device, add_user, app_with_tracks, call, conn};
use crate::db::testing::access;

/// The id of the queue [`start_queue`] starts.
const QUEUE: &str = "5f0c6a52-3b7e-4d43-9a8e-0d1c2b3a4f5e";

mod disconnected;
mod saved;

/// [`app_with_tracks`], served, with the owner also on device 2 as `phone`, and user 2, who
/// reaches only library 1, on device 3 as `user2`. Each account has a queue of tracks 1 and 2, as
/// [`start_queue`] starts, so its reports have items to name.
async fn setup() -> (tempfile::TempDir, Router, SocketAddr) {
    let (temp, _db, app) = app_with_tracks().await;
    let conn = conn(&temp);
    add_device(&conn, 1, 2, "phone");
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 3, "user2");
    access(&conn, 2, 1).unwrap();
    for token in [TOKEN, "user2"] {
        start_queue(&app, token).await;
    }
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
            "status": "stopped", "activeDeviceId": null, "itemId": null, "positionMs": 0,
            "volume": null,
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

    let changes = json!({ "itemId": item(1), "positionMs": 500, "start": true });
    let report = json!({ "type": "progress", "report": progress_report("playing", changes) });
    desk.send(Message::text(report.to_string())).await.unwrap();
    for socket in [&mut phone, &mut desk] {
        let state = &next_of(socket, "player").await["state"];
        assert_eq!((&state["activeDeviceId"], &state["itemId"]), (&json!("1"), &json!(item(1))));
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
    report(&mut desk, "playing", json!({})).await;
    assert_eq!(player(&app, TOKEN).await["status"], json!("stopped"), "none is active");

    report(&mut desk, "playing", json!({ "start": true, "volume": 0.5 })).await;
    let state = player(&app, TOKEN).await;
    assert_eq!(
        (&state["status"], &state["activeDeviceId"], &state["itemId"]),
        (&json!("playing"), &json!("1"), &json!(item(0)))
    );
    assert_eq!((&state["positionMs"], &state["volume"]), (&json!(1000), &json!(0.5)));

    // The phone starts, buffering first; the desk keeps reporting until it hears.
    let start = json!({ "start": true, "itemId": item(1), "positionMs": 0 });
    report(&mut phone, "buffering", start).await;
    report(&mut desk, "playing", json!({})).await;
    let state = player(&app, TOKEN).await;
    assert_eq!(
        (&state["status"], &state["activeDeviceId"], &state["itemId"], &state["volume"]),
        (&json!("paused"), &json!("2"), &json!(item(1)), &json!(null))
    );

    report(&mut phone, "stopped", json!({})).await;
    let state = player(&app, TOKEN).await;
    assert_eq!(
        (&state["status"], &state["activeDeviceId"], &state["positionMs"]),
        (&json!("stopped"), &json!(null), &json!(1000)),
        "where it stopped is kept"
    );
    report(&mut phone, "paused", json!({})).await;
    assert_eq!(player(&app, TOKEN).await["status"], json!("stopped"));
}

#[tokio::test]
async fn a_report_without_volume_keeps_the_playing_devices_volume() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    report(&mut desk, "playing", json!({ "start": true, "volume": 0.5 })).await;
    report(&mut desk, "paused", json!({})).await;
    assert_eq!(player(&app, TOKEN).await["volume"], json!(0.5));
}

#[tokio::test]
async fn reports_are_checked() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut user2 = connect_with(address, "user2").await;
    for changes in
        [json!({ "volume": 1.5 }), json!({ "positionMs": -1 }), json!({ "status": "rewinding" })]
    {
        let mut changes = changes;
        changes["start"] = json!(true);
        report(&mut desk, "playing", changes.clone()).await;
        assert_eq!(player(&app, TOKEN).await["status"], json!("stopped"), "{changes}");
    }
    start_playing(&mut user2).await;
    assert_eq!(player(&app, "user2").await["status"], json!("playing"));
}

#[tokio::test]
async fn report_with_unknown_item_refused() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let key = crate::api::testing::player::KEY;
    for item_id in ["", "item-1", key, &format!("{key}.2"), &format!("{key}.x"), &"x".repeat(5000)]
    {
        let changes = json!({ "itemId": item_id, "start": true });
        report(&mut desk, "playing", changes).await;
        let state = player(&app, TOKEN).await;
        assert_eq!(state["status"], json!("stopped"), "{item_id}");
        assert_eq!(state["itemId"], json!(null), "{item_id}");
    }
    // The queue's own items are accepted, whichever of its sections they are in.
    report(&mut desk, "playing", json!({ "itemId": item(1), "start": true })).await;
    assert_eq!(player(&app, TOKEN).await["itemId"], json!(item(1)));
    report(&mut desk, "playing", json!({ "itemId": item(0) })).await;
    assert_eq!(player(&app, TOKEN).await["itemId"], json!(item(0)));

    // An item the queue has lost is refused too, and the state keeps its place.
    let edit = json!({ "type": "remove", "items": [item(1)] });
    let body = json!({
        "editId": "7e0d1c52-3b7e-4d43-9a8e-0d1c2b3a0001", "queueId": QUEUE, "edit": edit,
    });
    let (status, _) = call(&app, TOKEN, "POST", "/api/v1/me/queue/edits", Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    report(&mut desk, "playing", json!({ "itemId": item(1) })).await;
    assert_eq!(player(&app, TOKEN).await["itemId"], json!(item(0)));
}

/// A queue is the account's own: an item of another account's, or of none, is not found.
#[tokio::test]
async fn an_item_of_another_accounts_queue_is_refused() {
    let (_temp, app, address) = setup().await;
    let other = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    let queue = json!({
        "queueId": "33333333-3333-4333-8333-333333333333", "context": context(),
        "start": 0, "source": { "key": other, "libraryId": "1", "tracks": ["1", "2"] },
    });
    let (status, _) = call(&app, "user2", "POST", "/api/v1/me/queue", Some(queue)).await;
    assert_eq!(status, StatusCode::OK);
    let mut desk = connect_with(address, TOKEN).await;
    let mut user2 = connect_with(address, "user2").await;
    let theirs = json!({ "itemId": format!("{other}.0"), "start": true });
    report(&mut desk, "playing", theirs.clone()).await;
    assert_eq!(player(&app, TOKEN).await["itemId"], json!(null), "the owner has no such item");
    report(&mut user2, "playing", theirs).await;
    assert_eq!(player(&app, "user2").await["itemId"], json!(format!("{other}.0")));
    report(&mut user2, "playing", json!({ "itemId": item(0) })).await;
    let state = player(&app, "user2").await;
    assert_eq!(state["itemId"], json!(format!("{other}.0")), "and theirs lost the old queue's");
    assert_eq!(player(&app, TOKEN).await["itemId"], json!(null), "the owner's state is theirs");
}

/// A `deviceId` in a report is ignored, whatever it names: a report is the device's whose
/// connection sends it.
#[tokio::test]
async fn a_reports_device_id_is_not_read() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    report(&mut desk, "playing", json!({ "start": true, "deviceId": "2" })).await;
    let state = player(&app, TOKEN).await;
    assert_eq!((&state["status"], &state["activeDeviceId"]), (&json!("playing"), &json!("1")));
}

#[tokio::test]
async fn of_a_devices_connections_only_the_one_playing_reports() {
    let (_temp, app, address) = setup().await;
    let mut tab = connect_with(address, TOKEN).await;
    let mut other_tab = connect_with(address, TOKEN).await;
    start_playing(&mut tab).await;
    report(&mut other_tab, "playing", json!({ "start": true, "itemId": item(1) })).await;
    report(&mut tab, "playing", json!({})).await;
    assert_eq!(player(&app, TOKEN).await["itemId"], json!(item(1)), "the tab that started");
}

#[tokio::test]
async fn a_report_is_checked_against_the_account_as_it_is_now() {
    let (temp, app, address) = setup().await;
    let mut admin = connect_with(address, TOKEN).await;
    start_playing(&mut admin).await;
    // Signed out by a path that does not close its connections.
    let mut phone = connect_with(address, "phone").await;
    let conn = conn(&temp);
    let revoke = "UPDATE devices SET revoked_reason = 'credentials_changed' WHERE id = 2";
    conn.execute(revoke, []).unwrap();
    start_playing(&mut phone).await;
    assert_eq!(player(&app, TOKEN).await["activeDeviceId"], json!("1"));

    // Suspended the same way. Its report is refused, and answered with its state.
    let mut user2 = connect_with(address, "user2").await;
    next_of(&mut user2, "player").await;
    conn.execute("UPDATE users SET status = 'suspended' WHERE id = 2", []).unwrap();
    let start = progress_report("playing", json!({ "start": true }));
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
    let refused = progress_report("playing", json!({}));
    let refused = json!({ "type": "progress", "report": refused });
    desk.send(Message::text(refused.to_string())).await.unwrap();
    let state = next_of(&mut desk, "player").await;
    assert_eq!(state["state"]["status"], json!("stopped"));
}

/// A device taking over names the item the state has, since it holds the account's queue, and the
/// state keeps it however the takeover goes; the device taken over from, still reporting before
/// it hears, changes nothing. A transfer carries no item.
#[tokio::test]
async fn takeover_keeps_item() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    let place = json!({ "start": true, "itemId": item(1), "positionMs": 2500 });
    report(&mut desk, "paused", place.clone()).await;

    let transfer = json!({ "type": "transfer", "commandId": command_id(1), "deviceId": "2" });
    let transfer = json!({ "type": "playerCommand", "command": transfer });
    desk.send(Message::text(transfer.to_string())).await.unwrap();
    let sent = next_of(&mut phone, "execute").await["command"].clone();
    assert_eq!(sent.get("itemId"), None);
    let state = player(&app, TOKEN).await;
    assert_eq!((&state["itemId"], &state["positionMs"]), (&json!(item(1)), &json!(2500)));

    // The phone carries it out by starting where the state says.
    report(&mut phone, "playing", place).await;
    let state = player(&app, TOKEN).await;
    assert_eq!((&state["activeDeviceId"], &state["itemId"]), (&json!("2"), &json!(item(1))));
    report(&mut desk, "playing", json!({ "itemId": item(0), "positionMs": 9000 })).await;
    let state = player(&app, TOKEN).await;
    assert_eq!((&state["itemId"], &state["positionMs"]), (&json!(item(1)), &json!(2500)));
    // Stopping, as when the phone's connection closes, leaves the item where it was.
    report(&mut phone, "stopped", json!({ "itemId": item(1), "positionMs": 3000 })).await;
    let state = player(&app, TOKEN).await;
    assert_eq!((&state["status"], &state["itemId"]), (&json!("stopped"), &json!(item(1))));
}
