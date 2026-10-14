use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::super::authenticate::Caller;
use super::super::realtime::Hub;
use super::super::testing::socket::{Socket, connect_with, next, next_of, serve};
use super::super::testing::{TOKEN, add_device, add_user, app_with_tracks, respond};
use super::{Id, Players};
use crate::db::Database;

/// [`app_with_tracks`], served, with the owner also on device 2 as `phone`, and user 2, who
/// reaches only library 1, on device 3 as `user2`.
async fn setup() -> (tempfile::TempDir, Router, SocketAddr) {
    let (temp, _db, app) = app_with_tracks().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_device(&conn, 1, 2, "phone");
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 3, "user2");
    conn.execute("INSERT INTO library_access VALUES (2, 1, 0)", []).unwrap();
    let address = serve(app.clone()).await;
    (temp, app, address)
}

/// Reports from `device` over `socket` that track 1 in library 1 is at `status`, changed by
/// `changes`, and returns once the server has handled it: a connection handles its messages
/// in order, so that is when the ping sent after it is answered.
async fn report(socket: &mut Socket, device: &str, status: &str, changes: Value) {
    let mut report = json!({
        "deviceId": device, "itemId": "item-1", "libraryId": "1", "trackId": "1",
        "status": status, "positionMs": 1000,
    });
    report.as_object_mut().unwrap().extend(changes.as_object().unwrap().clone());
    let progress = json!({ "type": "progress", "report": report });
    socket.send(Message::text(progress.to_string())).await.unwrap();
    socket.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    next_of(socket, "pong").await;
}

/// `getPlayer`, logged in with `token`.
async fn player(app: &Router, token: &str) -> Value {
    let request = Request::get("/api/v1/me/player")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = respond(app.clone(), request).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).unwrap()
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

    let report = json!({ "type": "progress", "report": {
        "deviceId": "1", "itemId": "item-1", "libraryId": "1", "trackId": "2",
        "status": "playing", "positionMs": 500, "start": true,
    } });
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
    report(&mut user2, "3", "playing", json!({ "start": true })).await;
    assert_eq!(player(&app, "user2").await["status"], json!("playing"), "library 1");
}

#[tokio::test]
async fn of_a_devices_connections_only_the_one_playing_reports() {
    let (_temp, app, address) = setup().await;
    let mut tab = connect_with(address, TOKEN).await;
    let mut other_tab = connect_with(address, TOKEN).await;
    report(&mut tab, "1", "playing", json!({ "start": true })).await;
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
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    conn.execute("DELETE FROM devices WHERE id = 2", []).unwrap();
    report(&mut phone, "2", "playing", json!({ "start": true })).await;
    assert_eq!(player(&app, TOKEN).await["activeDeviceId"], json!("1"));
}

#[tokio::test]
async fn a_refused_report_is_answered_with_the_current_state() {
    let (_temp, _app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    next_of(&mut desk, "player").await;
    // Not through `report`, whose wait for a pong would skip the answer if it came first.
    let refused = json!({ "type": "progress", "report": {
        "deviceId": "1", "itemId": "item-1", "libraryId": "1", "trackId": "1",
        "status": "playing", "positionMs": 0,
    } });
    desk.send(Message::text(refused.to_string())).await.unwrap();
    let state = next_of(&mut desk, "player").await;
    assert_eq!(state["state"]["status"], json!("stopped"));
}

/// Reports to `players` from connection 7 of device 1 of the owner that track `track` is at
/// `status` and `position_ms`.
fn report_to(players: &Players, status: &str, track: i64, position_ms: u32) {
    let report = serde_json::from_value(json!({
        "deviceId": "1", "itemId": "item-1", "libraryId": "1", "trackId": track.to_string(),
        "status": status, "positionMs": position_ms, "start": true,
    }));
    let caller = Caller { user: 1, device: 1, admin: true };
    players.report(&Hub::default(), caller, 7, report.unwrap(), Id(1), Id(track)).unwrap();
}

/// What `players` saved for the owner, as a restarted server reads it.
async fn restarted(db: &Database) -> Value {
    let mut state = serde_json::to_value(Players::load(db).await.unwrap().current(1)).unwrap();
    state.as_object_mut().unwrap().remove("positionAt");
    state
}

#[tokio::test]
async fn playback_resumes_where_it_was_after_a_restart() {
    let (_temp, db, _app) = app_with_tracks().await;
    let players = Players::default();
    report_to(&players, "playing", 1, 0);
    report_to(&players, "paused", 2, 2500);
    players.save(&db).await;
    assert_eq!(
        restarted(&db).await,
        json!({
            "status": "stopped", "activeDeviceId": null, "itemId": "item-1", "libraryId": "1",
            "trackId": "2", "positionMs": 2500, "volume": null,
        }),
        "no device is connected to be playing yet"
    );

    // Saved again only once something changes.
    db.write(|tx| tx.execute("DELETE FROM player_states", [])).await.unwrap();
    players.save(&db).await;
    assert_eq!(restarted(&db).await["trackId"], json!(null));
}

#[tokio::test]
async fn playing_is_saved_as_far_as_it_has_played() {
    let (_temp, db, _app) = app_with_tracks().await;
    let players = Players::default();
    report_to(&players, "playing", 1, 1000);
    players.lock().get_mut(&1).unwrap().position_at -= 3000;
    // As far as it has played: the 3 seconds put behind it, and however long the test took.
    let saved = async || restarted(&db).await["positionMs"].as_u64().unwrap();
    players.save(&db).await;
    assert!((4000..5000).contains(&saved().await));

    players.lock().get_mut(&1).unwrap().position_at -= 2000;
    players.save(&db).await;
    assert!((6000..7000).contains(&saved().await), "and again as it plays on");
}

#[tokio::test]
async fn a_saved_place_out_of_reach_is_not_resumed() {
    let (temp, db, _app) = app_with_tracks().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_user(&conn, 2, "user");
    conn.execute_batch(
        "INSERT INTO library_access VALUES (2, 1, 0);
         INSERT INTO player_states VALUES (2, 'item-1', 1, 1, 0), (1, 'item-1', 1, 99, 0);",
    )
    .unwrap();
    let players = Players::load(&db).await.unwrap();
    assert_eq!(players.current(2).track_id, Some(Id(1)), "reached");
    assert_eq!(players.current(1).track_id, None, "a track that is gone");

    conn.execute("DELETE FROM library_access", []).unwrap();
    assert_eq!(Players::load(&db).await.unwrap().current(2).track_id, None, "no longer reached");
}

#[tokio::test]
async fn one_accounts_place_failing_to_save_keeps_no_one_elses_from_saving() {
    let (temp, db, _app) = app_with_tracks().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_user(&conn, 2, "user");
    let players = Players::default();
    report_to(&players, "paused", 1, 1000);
    let caller = Caller { user: 2, device: 2, admin: false };
    let report = serde_json::from_value(json!({
        "deviceId": "2", "itemId": "item-1", "libraryId": "9", "trackId": "1",
        "status": "paused", "positionMs": 0, "start": true,
    }));
    // A library deleted after the report was taken.
    players.report(&Hub::default(), caller, 8, report.unwrap(), Id(9), Id(1)).unwrap();
    players.save(&db).await;
    assert_eq!(restarted(&db).await["positionMs"], json!(1000));
}

/// The next playback state `socket` is sent with `status`.
async fn state_with(socket: &mut Socket, status: &str) -> Value {
    loop {
        let message = next_of(socket, "player").await;
        if message["state"]["status"] == status {
            return message["state"].clone();
        }
    }
}

#[tokio::test]
async fn losing_the_playing_connection_stops_playback_where_it_got_to() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    next_of(&mut phone, "player").await; // As it was on connecting: stopped at nothing.
    report(&mut desk, "1", "playing", json!({ "start": true })).await;
    desk.close(None).await.unwrap();

    let state = state_with(&mut phone, "stopped").await;
    assert_eq!((&state["activeDeviceId"], &state["trackId"]), (&json!(null), &json!("1")));
    let position = state["positionMs"].as_u64().unwrap();
    assert!((1000..2000).contains(&position), "as far as it played: {position}");
    assert_eq!(player(&app, TOKEN).await, state);

    // Any device can carry on from there.
    report(&mut phone, "2", "playing", json!({ "start": true })).await;
    assert_eq!(player(&app, TOKEN).await["activeDeviceId"], json!("2"));
}

#[tokio::test]
async fn a_connection_that_drops_played_until_it_dropped() {
    let (_temp, _app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    next_of(&mut phone, "player").await; // As it was on connecting: stopped at nothing.
    report(&mut desk, "1", "playing", json!({ "start": true })).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    // Gone without a close frame, as when the network changes under it.
    drop(desk);

    let position = state_with(&mut phone, "stopped").await["positionMs"].as_u64().unwrap();
    assert!((1500..2500).contains(&position), "not rewound to its last report: {position}");
}

#[tokio::test]
async fn a_paused_place_is_kept_exactly_and_other_connections_closing_change_nothing() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    report(&mut desk, "1", "paused", json!({ "start": true })).await;

    phone.close(None).await.unwrap();
    // The phone's connection is let go before its device list is sent.
    loop {
        let list = next_of(&mut desk, "devices").await;
        if list["devices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["id"] == "2" && d["connected"] == false)
        {
            break;
        }
    }
    assert_eq!(player(&app, TOKEN).await["activeDeviceId"], json!("1"));

    let mut tab = connect_with(address, TOKEN).await;
    assert_eq!(next_of(&mut tab, "player").await["state"]["status"], json!("paused"));
    desk.close(None).await.unwrap();
    let state = state_with(&mut tab, "stopped").await;
    assert_eq!(state["positionMs"], json!(1000), "paused, so it got no further");
}

#[tokio::test]
async fn an_old_connection_closing_after_its_device_took_over_again_changes_nothing() {
    let (_temp, app, address) = setup().await;
    let mut old = connect_with(address, TOKEN).await;
    report(&mut old, "1", "playing", json!({ "start": true })).await;
    // Reconnected while the old connection lingers, and took over on the new one.
    let mut new = connect_with(address, TOKEN).await;
    report(&mut new, "1", "playing", json!({ "start": true, "trackId": "2" })).await;
    old.close(None).await.unwrap();
    while old.next().await.is_some() {}
    // Nothing can be waited on when nothing changes: give the server time to let go.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let state = player(&app, TOKEN).await;
    assert_eq!((&state["status"], &state["trackId"]), (&json!("playing"), &json!("2")));
}
