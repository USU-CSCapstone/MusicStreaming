//! Commands that fail while they wait: unanswered, or their device gone.

use std::time::Duration;

use serde_json::json;
use tokio::sync::oneshot;
use tokio::time::{Instant, timeout};

use super::super::Reply;
use super::super::relay::ANSWER_WITHIN;
use super::{OWNER, command, playing, result_of, send, setup};
use crate::api::testing::player::{command_id, start_playing};
use crate::api::testing::socket::{connect_with, next_of};
use crate::api::testing::{TOKEN, add_device, conn};

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
    start_playing(&mut desk).await;
    command(&mut phone, json!({ "type": "pause", "commandId": command_id(1) })).await;
    desk.close(None).await.unwrap();
    let result = result_of(&mut phone, &json!(command_id(1))).await;
    assert_eq!(result["reason"], json!("The device went away."));
}

#[tokio::test]
async fn a_command_ends_with_the_answer_of_the_device_it_went_to_even_after_another_starts() {
    let (temp, _app, address) = setup().await;
    add_device(&conn(&temp), 1, 4, "laptop");
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    let mut laptop = connect_with(address, "laptop").await;
    start_playing(&mut desk).await;
    command(&mut phone, json!({ "type": "pause", "commandId": command_id(1) })).await;
    start_playing(&mut laptop).await;

    // Only the connection it went to answers it, even once playback has moved on.
    let applied = json!({ "commandId": command_id(1), "status": "applied" });
    let refused = json!({ "commandId": command_id(1), "status": "failed", "reason": "No." });
    send(&mut laptop, json!({ "type": "commandResult", "result": refused })).await;
    send(&mut laptop, json!({ "type": "ping" })).await;
    next_of(&mut laptop, "pong").await;
    send(&mut desk, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(result_of(&mut phone, &json!(command_id(1))).await, applied);
}
