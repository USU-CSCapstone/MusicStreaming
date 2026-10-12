use std::sync::Arc;

use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::super::super::authenticate::Caller;
use super::super::super::realtime::Hub;
use super::super::super::testing::socket::{Socket, connect_with, next_of, serve};
use super::super::super::testing::{TOKEN, add_device, add_user, app_with_tracks};
use super::super::Players;
use super::{Id, MAX_WAITING};

/// Library 1's tracks, served, with the owner on device 1 as [`TOKEN`] and device 2 as
/// `phone`, and user 2 on device 3 as `stranger`.
async fn setup() -> (tempfile::TempDir, std::net::SocketAddr) {
    let (temp, _db, app) = app_with_tracks().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_device(&conn, 1, 2, "phone");
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 3, "stranger");
    (temp, serve(app).await)
}

async fn send(socket: &mut Socket, message: Value) {
    socket.send(Message::text(message.to_string())).await.unwrap();
}

/// Starts playback on `socket`, device `device`, and returns once the server has it.
async fn play(socket: &mut Socket, device: &str) {
    let report = json!({
        "deviceId": device, "itemId": "item-1", "libraryId": "1", "trackId": "1",
        "status": "playing", "positionMs": 0, "start": true,
    });
    send(socket, json!({ "type": "progress", "report": report })).await;
    send(socket, json!({ "type": "ping" })).await;
    next_of(socket, "pong").await;
}

fn id(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// The next result `socket` is sent for the command `command_id`, skipping others'.
async fn result_of(socket: &mut Socket, command_id: &Value) -> Value {
    loop {
        let result = next_of(socket, "commandResult").await["result"].clone();
        if result["commandId"] == *command_id {
            return result;
        }
    }
}

/// Sends `command` over `socket` and returns what it is told at once.
async fn command(socket: &mut Socket, command: Value) -> Value {
    send(socket, json!({ "type": "playerCommand", "command": command.clone() })).await;
    result_of(socket, &command["commandId"]).await
}

#[tokio::test]
async fn a_command_reaches_the_device_playing_and_its_answer_comes_back() {
    let (_temp, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    play(&mut desk, "1").await;

    let seek = json!({ "type": "seek", "commandId": id(1), "positionMs": 30000 });
    let told = command(&mut phone, seek.clone()).await;
    assert_eq!(told, json!({ "commandId": id(1), "status": "pending" }));
    assert_eq!(next_of(&mut desk, "execute").await["command"], seek);

    let applied = json!({ "commandId": id(1), "status": "applied" });
    send(&mut desk, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(result_of(&mut phone, &json!(id(1))).await, applied);
}

#[tokio::test]
async fn only_the_device_it_went_to_answers_a_command() {
    let (_temp, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    play(&mut desk, "1").await;
    command(&mut phone, json!({ "type": "pause", "commandId": id(1) })).await;

    let claimed = json!({ "commandId": id(1), "status": "applied" });
    send(&mut phone, json!({ "type": "commandResult", "result": claimed })).await;
    let failed = json!({ "commandId": id(1), "status": "failed", "reason": "Could not pause." });
    send(&mut desk, json!({ "type": "commandResult", "result": failed })).await;
    assert_eq!(result_of(&mut phone, &json!(id(1))).await, failed);
}

#[tokio::test]
async fn a_command_with_nowhere_to_go_fails_at_once() {
    let (_temp, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    let mut stranger = connect_with(address, "stranger").await;
    let pause = |n| json!({ "type": "pause", "commandId": id(n) });
    let reason = |result: Value| (result["status"].clone(), result["reason"].clone());

    let nothing = (json!("failed"), json!("Nothing is playing."));
    assert_eq!(reason(command(&mut phone, pause(1)).await), nothing);
    play(&mut desk, "1").await;
    assert_eq!(reason(command(&mut stranger, pause(2)).await), nothing, "another account's");

    let elsewhere =
        json!({ "type": "setVolume", "commandId": id(3), "deviceId": "2", "volume": 0.5 });
    let not_playing = (json!("failed"), json!("That device is not the one playing."));
    assert_eq!(reason(command(&mut phone, elsewhere).await), not_playing);
    let loud = json!({ "type": "setVolume", "commandId": id(4), "deviceId": "1", "volume": 2 });
    assert_eq!(command(&mut phone, loud).await["status"], json!("failed"));
    let unnamed = json!({ "type": "pause", "commandId": "not-a-uuid" });
    assert_eq!(command(&mut phone, unnamed).await["status"], json!("failed"));
}

#[tokio::test]
async fn a_command_the_device_playing_does_not_answer_fails() {
    let (_temp, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    play(&mut desk, "1").await;
    let told = command(&mut phone, json!({ "type": "next", "commandId": id(1) })).await;
    assert_eq!(told["status"], json!("pending"));
    let result = result_of(&mut phone, &json!(id(1))).await;
    assert_eq!(
        (&result["status"], &result["reason"]),
        (&json!("failed"), &json!("The device playing did not answer."))
    );
}

#[tokio::test]
async fn an_account_can_have_only_so_many_commands_waiting() {
    let (players, hub) = (Arc::new(Players::default()), Arc::new(Hub::default()));
    let caller = Caller { user: 1, device: 1, admin: true };
    let report = serde_json::from_value(json!({
        "deviceId": "1", "itemId": "item-1", "libraryId": "1", "trackId": "1",
        "status": "playing", "positionMs": 0, "start": true,
    }));
    players.report(&hub, caller, 7, report.unwrap(), Id(1), Id(1)).unwrap();
    let command = |n: u32| {
        let command = serde_json::from_value(json!({ "type": "play", "commandId": id(n) }));
        let result = players.command(&hub, caller, 8, command.unwrap());
        serde_json::to_value(result).unwrap()["status"].clone()
    };
    for n in 0..MAX_WAITING as u32 {
        assert_eq!(command(n), json!("pending"), "{n}");
    }
    assert_eq!(command(99), json!("failed"));
    assert_eq!(command(0), json!("pending"), "sent again, it is still the one waiting");
}

#[tokio::test]
async fn a_command_this_server_does_not_carry_out_fails_at_once() {
    let (_temp, address) = setup().await;
    let mut phone = connect_with(address, "phone").await;
    let transfer = json!({ "type": "transfer", "commandId": id(1), "deviceId": "1" });
    assert_eq!(command(&mut phone, transfer).await["status"], json!("failed"));
    let backwards = json!({ "type": "seek", "commandId": id(2), "positionMs": -1 });
    assert_eq!(command(&mut phone, backwards).await["status"], json!("failed"));
    let quiet = json!({ "type": "setVolume", "commandId": id(3), "volume": 0.5 });
    assert_eq!(command(&mut phone, quiet).await["status"], json!("failed"), "needs a device");
}

#[tokio::test]
async fn a_command_sent_again_after_reconnecting_is_answered_on_the_new_connection() {
    let (_temp, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    play(&mut desk, "1").await;
    let mut phone = connect_with(address, "phone").await;
    let pause = json!({ "type": "pause", "commandId": id(1) });
    command(&mut phone, pause.clone()).await;
    phone.close(None).await.unwrap();

    let mut phone = connect_with(address, "phone").await;
    assert_eq!(command(&mut phone, pause).await["status"], json!("pending"));
    next_of(&mut desk, "execute").await;
    let applied = json!({ "commandId": id(1), "status": "applied" });
    send(&mut desk, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(result_of(&mut phone, &json!(id(1))).await, applied);
}

#[tokio::test]
async fn two_accounts_may_use_the_same_command_id() {
    let (temp, address) = setup().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    conn.execute("INSERT INTO library_access VALUES (2, 1, 0)", []).unwrap();
    let mut desk = connect_with(address, TOKEN).await;
    let mut stranger = connect_with(address, "stranger").await;
    play(&mut desk, "1").await;
    play(&mut stranger, "3").await;
    let pause = json!({ "type": "pause", "commandId": id(1) });
    for socket in [&mut desk, &mut stranger] {
        assert_eq!(command(socket, pause.clone()).await["status"], json!("pending"));
        assert_eq!(next_of(socket, "execute").await["command"], pause, "each carried out");
    }
}

#[tokio::test]
async fn commands_waiting_on_a_device_that_goes_away_fail_at_once() {
    let (_temp, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    play(&mut desk, "1").await;
    command(&mut phone, json!({ "type": "pause", "commandId": id(1) })).await;
    desk.close(None).await.unwrap();
    let result = result_of(&mut phone, &json!(id(1))).await;
    assert_eq!(result["reason"], json!("The device playing went away."));
}
