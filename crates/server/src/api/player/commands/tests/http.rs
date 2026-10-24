//! Commands over HTTP, answered with how they went, and commands sent again.

use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{command, result_of, send, setup};
use crate::api::testing::player::{command_id, start_playing};
use crate::api::testing::socket::{connect_with, next_of};
use crate::api::testing::{TOKEN, call};

/// Sends `command` over HTTP, logged in with `token`.
async fn post(app: &Router, token: &str, command: Value) -> (StatusCode, Value) {
    call(app, token, "POST", "/api/v1/me/player/commands", Some(command)).await
}

#[tokio::test]
async fn a_command_over_http_is_answered_with_how_it_went() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    start_playing(&mut desk).await;
    let pause = json!({ "type": "pause", "commandId": command_id(1) });
    let sent = tokio::spawn({
        let (app, pause) = (app.clone(), pause.clone());
        async move { post(&app, "phone", pause).await }
    });
    assert_eq!(next_of(&mut desk, "execute").await["command"], pause);
    let applied = json!({ "commandId": command_id(1), "status": "applied" });
    send(&mut desk, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(sent.await.unwrap(), (StatusCode::OK, applied));
}

#[tokio::test]
async fn a_command_over_http_with_nowhere_to_go_is_refused() {
    let (_temp, app, address) = setup().await;
    let (status, problem) =
        post(&app, "phone", json!({ "type": "play", "commandId": command_id(1) })).await;
    assert_eq!((status, &problem["code"]), (StatusCode::CONFLICT, &json!("device_unavailable")));

    let mut desk = connect_with(address, TOKEN).await;
    start_playing(&mut desk).await;
    for invalid in [
        json!({ "type": "setVolume", "commandId": command_id(2), "deviceId": "1", "volume": 2 }),
        json!({ "type": "rewind", "commandId": command_id(3) }),
        json!({ "type": "pause", "commandId": "not-a-uuid" }),
    ] {
        let (status, problem) = post(&app, "phone", invalid.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{invalid}: {problem}");
    }
}

#[tokio::test]
async fn a_command_sent_again_after_it_was_answered_is_answered_again_not_carried_out() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    start_playing(&mut desk).await;
    let next = json!({ "type": "next", "commandId": command_id(1) });
    command(&mut phone, next.clone()).await;
    next_of(&mut desk, "execute").await;
    let applied = json!({ "commandId": command_id(1), "status": "applied" });
    send(&mut desk, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(result_of(&mut phone, &json!(command_id(1))).await, applied);

    assert_eq!(command(&mut phone, next.clone()).await, applied, "at once, not pending");
    assert_eq!(post(&app, "phone", next).await, (StatusCode::OK, applied));
}

#[tokio::test]
async fn a_command_id_names_the_same_command_in_capitals() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    start_playing(&mut desk).await;
    let id = "abcdef00-0000-4000-8000-000000000001";
    let shouted = json!({ "type": "next", "commandId": id.to_uppercase() });
    let first = tokio::spawn({
        let app = app.clone();
        async move { post(&app, "phone", shouted).await }
    });
    let executed = next_of(&mut desk, "execute").await;
    assert_eq!(executed["command"]["commandId"], json!(id), "passed on in lowercase");
    let applied = json!({ "commandId": id, "status": "applied" });
    send(&mut desk, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(first.await.unwrap(), (StatusCode::OK, applied.clone()));
    let again = json!({ "type": "next", "commandId": id });
    assert_eq!(post(&app, "phone", again).await, (StatusCode::OK, applied), "not carried out");
}

#[tokio::test]
async fn a_device_may_answer_a_command_with_its_id_in_capitals() {
    let (_temp, _app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, "phone").await;
    start_playing(&mut desk).await;
    let id = "abcdef00-0000-4000-8000-000000000002";
    command(&mut phone, json!({ "type": "pause", "commandId": id })).await;
    next_of(&mut desk, "execute").await;
    let shouted = json!({ "commandId": id.to_uppercase(), "status": "applied" });
    send(&mut desk, json!({ "type": "commandResult", "result": shouted })).await;
    let applied = json!({ "commandId": id, "status": "applied" });
    assert_eq!(result_of(&mut phone, &json!(id)).await, applied);
}

#[tokio::test]
async fn a_command_sent_again_while_it_waits_is_answered_where_it_was_sent_last() {
    let (_temp, app, address) = setup().await;
    let mut desk = connect_with(address, TOKEN).await;
    start_playing(&mut desk).await;
    let pause = json!({ "type": "pause", "commandId": command_id(1) });
    let first = tokio::spawn({
        let (app, pause) = (app.clone(), pause.clone());
        async move { post(&app, "phone", pause).await }
    });
    next_of(&mut desk, "execute").await;
    let again = tokio::spawn({
        let (app, pause) = (app.clone(), pause.clone());
        async move { post(&app, "phone", pause).await }
    });
    let pending = json!({ "commandId": command_id(1), "status": "pending" });
    assert_eq!(first.await.unwrap(), (StatusCode::OK, pending), "displaced by the second");

    let applied = json!({ "commandId": command_id(1), "status": "applied" });
    send(&mut desk, json!({ "type": "commandResult", "result": applied })).await;
    assert_eq!(again.await.unwrap(), (StatusCode::OK, applied));
}
