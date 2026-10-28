//! A connection closing: playback stops where it got to if it was the one playing.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{player, setup};
use crate::api::testing::player::{item, report, start_playing};
use crate::api::testing::socket::{Socket, connect_with, next_of};
use crate::api::testing::{TOKEN, conn, eventually};

/// The furthest a report of playing from 1000 ms, sent at `reported`, can have got by now. The
/// server subtracts whole-millisecond wall-clock times, which can count one more than has passed.
fn furthest(reported: Instant) -> u64 {
    1001 + u64::try_from(reported.elapsed().as_millis()).unwrap()
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
    let reported = Instant::now();
    start_playing(&mut desk).await;
    desk.close(None).await.unwrap();

    let state = state_with(&mut phone, "stopped").await;
    assert_eq!((&state["activeDeviceId"], &state["itemId"]), (&json!(null), &json!(item(0))));
    let position = state["positionMs"].as_u64().unwrap();
    assert!((1000..=furthest(reported)).contains(&position), "as far as it played: {position}");
    assert_eq!(player(&app, TOKEN).await, state);

    // Any device can carry on from there.
    start_playing(&mut phone).await;
    assert_eq!(player(&app, TOKEN).await["activeDeviceId"], json!("2"));
}

#[tokio::test]
async fn a_connection_that_drops_played_until_it_dropped() {
    let (_temp, _app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    next_of(&mut phone, "player").await; // As it was on connecting: stopped at nothing.
    let reported = Instant::now();
    start_playing(&mut desk).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Gone without a close frame, as when the network changes under it.
    drop(desk);

    let position = state_with(&mut phone, "stopped").await["positionMs"].as_u64().unwrap();
    let played = 1500..=furthest(reported);
    assert!(played.contains(&position), "not rewound to its last report: {position}");
}

#[tokio::test]
async fn a_paused_place_is_kept_exactly_and_other_connections_closing_change_nothing() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    report(&mut desk, "paused", json!({ "start": true })).await;

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
    let (temp, app, address) = setup().await;
    let mut old = connect_with(address, TOKEN).await;
    start_playing(&mut old).await;
    // Reconnected while the old connection lingers, and took over on the new one.
    let mut new = connect_with(address, TOKEN).await;
    report(&mut new, "playing", json!({ "start": true, "itemId": item(1) })).await;
    // Once the server has let a connection go, it records when its device was last seen. Seen
    // in 2100 until then, so only that write lowers it: a request skips refreshing a device
    // seen so recently.
    let conn = conn(&temp);
    let later = 4_102_444_800_000_i64;
    conn.execute("UPDATE devices SET last_seen_at = ?1 WHERE id = 1", [later]).unwrap();
    old.close(None).await.unwrap();
    let let_go = async || {
        let seen = "SELECT last_seen_at FROM devices WHERE id = 1";
        let at: i64 = conn.query_row(seen, [], |row| row.get(0)).unwrap();
        (at < later).then_some(())
    };
    eventually("the old connection to be let go", let_go).await;
    let state = player(&app, TOKEN).await;
    assert_eq!((&state["status"], &state["itemId"]), (&json!("playing"), &json!(item(1))));
}
