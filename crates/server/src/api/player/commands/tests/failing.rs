//! Commands that fail while they wait: unanswered, their device gone, or playback taken over.

use std::time::Duration;

use serde_json::json;
use tokio::sync::oneshot;
use tokio::time::{Instant, timeout};

use super::super::Reply;
use super::super::relay::ANSWER_WITHIN;
use super::{OWNER, command, playing, result_of, send, setup};
use crate::api::testing::player::{command_id, report, start_playing};
use crate::api::testing::socket::{connect_with, next_of};
use crate::api::testing::{TOKEN, add_device};

// Paused time, which runs ahead whenever every task waits: no socket here waits on it.
#[tokio::test(start_paused = true)]
async fn a_command_the_device_playing_does_not_answer_fails() {
    let (players, hub) = playing();
    let next = serde_json::from_value(json!({ "type": "next", "commandId": command_id(1) }));
    let (reply, answer) = oneshot::channel();
    let sent = Instant::now();
    let told = players.command(&hub, OWNER, Reply::Request(reply), next.unwrap()).unwrap();
    assert_eq!(serde_json::to_value(told).unwrap()["status"], json!("pending"));
    let within = ANSWER_WITHIN + Duration::from_secs(1);
    let result = timeout(within, answer).await.expect("in its time").unwrap();
    let result = serde_json::to_value(result).unwrap();
    assert!(sent.elapsed() >= ANSWER_WITHIN, "not before its time");
    assert_eq!(
        (&result["status"], &result["reason"]),
        (&json!("failed"), &json!("The device did not answer."))
    );
}

#[tokio::test]
async fn commands_waiting_on_a_device_that_goes_away_fail_at_once() {
    let (_temp, _app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    start_playing(&mut desk, "1").await;
    command(&mut phone, json!({ "type": "pause", "commandId": command_id(1) })).await;
    desk.close(None).await.unwrap();
    let result = result_of(&mut phone, &json!(command_id(1))).await;
    assert_eq!(result["reason"], json!("The device went away."));
}

#[tokio::test]
async fn commands_waiting_on_a_device_taken_over_from_fail_at_once() {
    let (temp, _app, address) = setup().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_device(&conn, 1, 4, "laptop");
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    let mut laptop = connect_with(address, "laptop").await;
    start_playing(&mut desk, "1").await;
    let pause = |n| json!({ "type": "pause", "commandId": command_id(n) });

    // The device playing reports, even starting again, and is still the one to answer.
    command(&mut phone, pause(1)).await;
    start_playing(&mut desk, "1").await;
    let applied = json!({ "commandId": command_id(1), "status": "applied" });
    send(&mut desk, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(result_of(&mut phone, &json!(command_id(1))).await, applied);

    command(&mut phone, pause(2)).await;
    start_playing(&mut laptop, "4").await;
    let failed = json!({
        "commandId": command_id(2), "status": "failed", "reason": "Playback was taken over.",
    });
    assert_eq!(result_of(&mut phone, &json!(command_id(2))).await, failed);
    // Its answer, too late, counts for nothing.
    let late = json!({ "commandId": command_id(2), "status": "applied" });
    send(&mut desk, json!({ "type": "commandResult", "result": late })).await;
    send(&mut desk, json!({ "type": "ping" })).await;
    next_of(&mut desk, "pong").await;
    assert_eq!(command(&mut phone, pause(2)).await, failed, "answered again, not carried out");

    // Reports that do not start leave commands waiting: one refused from the device taken over
    // from, and one from the device playing while a transfer elsewhere waits.
    command(&mut phone, pause(3)).await;
    report(&mut desk, "1", "playing", json!({})).await;
    let applied = json!({ "commandId": command_id(3), "status": "applied" });
    send(&mut laptop, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(result_of(&mut phone, &json!(command_id(3))).await, applied);
    let transfer = json!({ "type": "transfer", "commandId": command_id(4), "deviceId": "1" });
    command(&mut phone, transfer).await;
    report(&mut laptop, "4", "playing", json!({})).await;
    let applied = json!({ "commandId": command_id(4), "status": "applied" });
    send(&mut desk, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(result_of(&mut phone, &json!(command_id(4))).await, applied);
}

#[tokio::test]
async fn commands_waiting_on_a_device_that_stopped_fail_once_playback_starts_elsewhere() {
    let (temp, _app, address) = setup().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_device(&conn, 1, 4, "laptop");
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    let mut laptop = connect_with(address, "laptop").await;
    start_playing(&mut desk, "1").await;
    command(&mut phone, json!({ "type": "play", "commandId": command_id(1) })).await;
    report(&mut desk, "1", "stopped", json!({})).await;
    start_playing(&mut laptop, "4").await;
    let result = result_of(&mut phone, &json!(command_id(1))).await;
    assert_eq!(
        (&result["status"], &result["reason"]),
        (&json!("failed"), &json!("Playback was taken over."))
    );
}
