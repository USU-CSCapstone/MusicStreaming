//! `setVolume`, which goes to the device it names, playing or not: volume belongs to the device
//! (`requirements/realtime.md` §3).

use serde_json::{Value, json};

use super::{Socket, command, next, result_of, send, setup};
use crate::api::testing::player::{command_id, start_playing};
use crate::api::testing::socket::{connect_with, next_of};
use crate::api::testing::{TOKEN, add_device, conn};

/// A `setVolume` command `n` for device `device`.
fn volume(n: u32, device: &str) -> Value {
    json!({ "type": "setVolume", "commandId": command_id(n), "deviceId": device, "volume": 0.5 })
}

/// Whether `socket` was sent a command before it answers a ping sent now.
async fn executed(socket: &mut Socket) -> bool {
    send(socket, json!({ "type": "ping" })).await;
    let mut executed = false;
    loop {
        let message = next(socket).await;
        executed |= message["type"] == json!("execute");
        if message["type"] == json!("pong") {
            return executed;
        }
    }
}

#[tokio::test]
async fn a_volume_reaches_the_device_it_names_though_it_is_not_playing() {
    let (_temp, _app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;

    assert_eq!(command(&mut phone, volume(1, "1")).await["status"], json!("pending"));
    assert_eq!(next_of(&mut desk, "execute").await["command"], volume(1, "1"), "nothing plays");

    start_playing(&mut desk).await;
    assert_eq!(command(&mut desk, volume(2, "2")).await["status"], json!("pending"));
    assert_eq!(next_of(&mut phone, "execute").await["command"], volume(2, "2"));
    let applied = json!({ "commandId": command_id(2), "status": "applied" });
    send(&mut phone, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(result_of(&mut desk, &json!(command_id(2))).await, applied);
}

/// A device that is not playing hears it on its newest connection.
#[tokio::test]
async fn a_volume_for_a_device_not_playing_goes_to_its_newest_connection() {
    let (_temp, _app, address) = setup().await;
    let mut older_tab = connect_with(address, TOKEN).await;
    let mut newer_tab = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;

    assert_eq!(command(&mut phone, volume(1, "1")).await["status"], json!("pending"));
    assert_eq!(next_of(&mut newer_tab, "execute").await["command"], volume(1, "1"));
    assert!(!executed(&mut older_tab).await);
}

/// The device playing hears it on the connection playing, though a newer one of its is open.
#[tokio::test]
async fn a_volume_for_the_device_playing_goes_to_the_connection_playing() {
    let (_temp, _app, address) = setup().await;
    let mut tab = connect_with(address, TOKEN).await;
    start_playing(&mut tab).await;
    let mut newer_tab = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;

    assert_eq!(command(&mut phone, volume(1, "1")).await["status"], json!("pending"));
    assert_eq!(next_of(&mut tab, "execute").await["command"], volume(1, "1"));
    assert!(!executed(&mut newer_tab).await);
}

#[tokio::test]
async fn a_volume_for_a_device_not_connected_fails_at_once() {
    let (temp, _app, address) = setup().await;
    add_device(&conn(&temp), 1, 4, "laptop");
    let mut desk = connect_with(address, TOKEN).await;
    let _stranger = connect_with(address, "stranger").await;
    start_playing(&mut desk).await;
    let not_connected = json!("That device is not connected.");
    for (n, device) in [(1, "4"), (2, "3"), (3, "x")] {
        let result = command(&mut desk, volume(n, device)).await;
        assert_eq!((&result["status"], &result["reason"]), (&json!("failed"), &not_connected));
    }
    assert!(!executed(&mut desk).await, "not even the device playing");
}
