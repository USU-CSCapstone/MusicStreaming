//! A signed-out device: refused as it connects and told why, not signed in once joined, and a
//! connection the hub has closed acts for no one, whatever its client sent before it heard.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::Message;
use axum::http::StatusCode;
use axum::{Json, Router};
use rusqlite::types::{FromSql, ValueRef};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Error;

use super::super::super::AppState;
use super::super::super::player;
use super::super::super::scope::Caller;
use super::super::super::testing::player::{command_id, progress_report};
use super::super::super::testing::socket::{connect, serve};
use super::super::super::testing::{add_signed_out_device, app, app_with_tracks, conn, state};
use super::super::{Presence, Revoked, handle, signed_in};
use crate::db::{Database, Text};

const OWNER: Caller = Caller { user: 1, device: 1, admin: true };

/// The messages the hub has queued for `presence`, without waiting for more.
async fn queued(presence: &mut Presence) -> Vec<Message> {
    let mut messages = Vec::new();
    while let Ok(Some(message)) = tokio::time::timeout(Duration::ZERO, presence.next()).await {
        messages.push(message);
    }
    messages
}

/// The owner's playback state, as `getPlayer` answers it.
async fn playback(state: &AppState) -> Value {
    let Json(current) = player::get(State(state.players.clone()), OWNER).await;
    serde_json::to_value(current).unwrap()
}

#[tokio::test]
async fn a_revoked_connection_acts_for_no_one() {
    let (temp, db, _app) = app_with_tracks().await;
    let state = state(&temp, db);
    let phone = Caller { device: 2, ..OWNER };
    let mut other = state.players.connect(&state.hub, 1, phone.device);
    let revoked = state.players.connect(&state.hub, 1, OWNER.device);
    let progress = |start: bool| {
        let changes = json!({ "positionMs": if start { 0 } else { 5000 }, "start": start });
        json!({ "type": "progress", "report": progress_report("1", "playing", changes) })
    };
    // The connection plays, and is sent a command from the phone to carry out.
    let play = progress(true).to_string();
    assert!(handle(&state, OWNER, &revoked, &play).await.is_none());
    let pause = json!({ "commandId": command_id(1), "type": "pause" });
    let pause = json!({ "type": "playerCommand", "command": pause }).to_string();
    let told = handle(&state, phone, &other, &pause).await;
    let pending = json!({ "commandId": command_id(1), "status": "pending" });
    assert_eq!(serde_json::to_value(told).unwrap()["result"], pending);
    queued(&mut other).await;
    let before = playback(&state).await;
    assert_eq!(before["activeDeviceId"], json!("1"));

    // The device's token is left in place, so only the closed connection stands in the way.
    state.hub.revoke(1, OWNER.device, Revoked::LoggedOut);
    let applied = json!({ "commandId": command_id(1), "status": "applied" });
    let transfer = json!({ "commandId": command_id(2), "type": "transfer", "deviceId": "2" });
    for message in [
        progress(false),
        json!({ "type": "commandResult", "result": applied }),
        json!({ "type": "playerCommand", "command": transfer }),
    ] {
        let reply = handle(&state, OWNER, &revoked, &message.to_string()).await;
        assert!(reply.is_none(), "{message}");
    }
    assert_eq!(queued(&mut other).await, Vec::<Message>::new(), "no state, result or command");
    assert_eq!(playback(&state).await, before);
}

/// [`app`] with the owner also on device 2, with token `revoked`, signed out when the
/// account's credentials changed.
fn with_signed_out_device() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app("");
    let conn = conn(&temp);
    add_signed_out_device(&conn, 1, 2, "revoked", "credentials_changed");
    (temp, db, app)
}

#[tokio::test]
async fn a_signed_out_device_is_refused_as_it_connects_and_told_why() {
    let (_temp, _db, app) = with_signed_out_device();
    let address = serve(app).await;
    let refused = connect(address, &[("authorization", "Bearer revoked")]).await.unwrap_err();
    let Error::Http(response) = refused else { panic!("{refused}") };
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let problem: Value = serde_json::from_slice(response.body().as_deref().unwrap()).unwrap();
    assert_eq!(
        (&problem["code"], &problem["reason"]),
        (&json!("session_revoked"), &json!("credentials_changed"))
    );
}

#[tokio::test]
async fn a_signed_out_device_is_not_signed_in_once_joined() {
    let (_temp, db, _app) = with_signed_out_device();
    assert!(signed_in(&db, 1).await.unwrap(), "the owner's other device");
    assert!(!signed_in(&db, 2).await.unwrap());
}

#[test]
fn a_stored_reason_reads_by_its_spec_name() {
    let read = |name: &[u8]| Text::<Revoked>::column_result(ValueRef::Text(name)).map(|r| r.0);
    assert!(matches!(read(b"credentials_changed"), Ok(Revoked::CredentialsChanged)));
    assert!(matches!(read(b"logged_out"), Ok(Revoked::LoggedOut)));
    let unknown = read(b"unknown").unwrap_err().to_string();
    assert!(unknown.contains("unknown variant"), "{unknown}");
}
