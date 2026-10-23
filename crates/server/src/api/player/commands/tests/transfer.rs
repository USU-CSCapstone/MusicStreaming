//! `transfer`, which goes to the device it names rather than the one playing.

use serde_json::{Value, json};

use super::{command, result_of, send, setup};
use crate::api::testing::player::{command_id, report, start_playing};
use crate::api::testing::socket::{connect_with, next_of};
use crate::api::testing::{TOKEN, add_device, conn};

#[tokio::test]
async fn a_transfer_goes_to_the_device_it_names_and_its_answer_comes_back() {
    let (_temp, _app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    start_playing(&mut desk, "1").await;

    let transfer = json!({ "type": "transfer", "commandId": command_id(1), "deviceId": "2" });
    assert_eq!(command(&mut desk, transfer).await["status"], json!("pending"));
    let sent = next_of(&mut phone, "execute").await["command"].clone();
    assert_eq!(
        sent,
        json!({ "type": "transfer", "commandId": command_id(1), "deviceId": "2", "play": true })
    );
    // It takes over, as a client carrying it out does, before it answers.
    start_playing(&mut phone, "2").await;
    let applied = json!({ "commandId": command_id(1), "status": "applied" });
    send(&mut phone, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(result_of(&mut desk, &json!(command_id(1))).await, applied);

    let here = json!({ "type": "transfer", "commandId": command_id(2), "deviceId": "2" });
    assert_eq!(command(&mut desk, here).await["status"], json!("applied"), "already there");
}

#[tokio::test]
async fn a_transfer_to_play_where_it_is_paused_plays_it() {
    let (_temp, _app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    start_playing(&mut desk, "1").await;
    report(&mut desk, "1", "paused", json!({})).await;

    let transfer = json!({ "type": "transfer", "commandId": command_id(1), "deviceId": "1" });
    assert_eq!(command(&mut phone, transfer).await["status"], json!("pending"));
    let sent = next_of(&mut desk, "execute").await["command"].clone();
    assert_eq!(sent, json!({ "type": "play", "commandId": command_id(1), "deviceId": "1" }));

    // Sent to the connection playing, it fails once playback starts on another.
    let mut tab = connect_with(address, TOKEN).await;
    start_playing(&mut tab, "1").await;
    let result = result_of(&mut phone, &json!(command_id(1))).await;
    assert_eq!(result["reason"], json!("Playback was taken over."));
}

#[tokio::test]
async fn a_transfer_already_sent_is_left_to_finish_when_playback_starts_elsewhere() {
    let (temp, _app, address) = setup().await;
    let conn = conn(&temp);
    add_device(&conn, 1, 4, "laptop");
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    let mut laptop = connect_with(address, "laptop").await;
    start_playing(&mut desk, "1").await;
    let transfer = json!({ "type": "transfer", "commandId": command_id(1), "deviceId": "4" });
    assert_eq!(command(&mut phone, transfer).await["status"], json!("pending"));
    next_of(&mut laptop, "execute").await;

    // The desk reconnects and starts again on its new connection, before the laptop takes over.
    let mut desk_again = connect_with(address, TOKEN).await;
    start_playing(&mut desk_again, "1").await;
    start_playing(&mut laptop, "4").await;
    let applied = json!({ "commandId": command_id(1), "status": "applied" });
    send(&mut laptop, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(result_of(&mut phone, &json!(command_id(1))).await, applied);
}

#[tokio::test]
async fn a_transfer_wakes_a_device_once_playback_stopped() {
    let (_temp, _app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    start_playing(&mut desk, "1").await;
    desk.close(None).await.unwrap();
    let mut phone = connect_with(address, "phone").await;
    let mut laptop = connect_with(address, TOKEN).await;

    let paused =
        json!({ "type": "transfer", "commandId": command_id(1), "deviceId": "2", "play": false });
    assert_eq!(command(&mut laptop, paused).await["status"], json!("pending"));
    assert_eq!(next_of(&mut phone, "execute").await["command"]["play"], json!(false));
}

#[tokio::test]
async fn a_transfer_needs_a_device_of_the_account_that_is_connected_and_something_to_move() {
    let (temp, _app, address) = setup().await;
    let conn = conn(&temp);
    add_device(&conn, 1, 4, "tablet");
    let mut desk = connect_with(address, TOKEN).await;
    // Another account's device, connected, which this one still cannot reach.
    let _stranger = connect_with(address, "stranger").await;
    let transfer = |n, device: &str| {
        let id = command_id(n);
        json!({ "type": "transfer", "commandId": id, "deviceId": device })
    };
    let reason = |result: Value| result["reason"].as_str().unwrap_or_default().to_owned();

    assert_eq!(reason(command(&mut desk, transfer(1, "4")).await), "Nothing has played to move.");
    start_playing(&mut desk, "1").await;
    for (n, device) in [(2, "4"), (3, "3"), (4, "nonsense")] {
        let result = command(&mut desk, transfer(n, device)).await;
        assert_eq!(reason(result), "That device is not connected.", "{device}");
    }
    let nameless = json!({ "type": "transfer", "commandId": command_id(5) });
    assert_eq!(command(&mut desk, nameless).await["status"], json!("failed"));
}
