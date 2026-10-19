//! A connection the hub has closed acts for no one, whatever its client sent before it heard.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::extract::ws::Message;
use serde_json::{Value, json};

use super::super::super::authenticate::Caller;
use super::super::super::player;
use super::super::super::testing::app_with_tracks;
use super::super::super::{AppState, Images};
use super::super::{Presence, Revoked, handle};
use crate::plugins::Plugins;

const OWNER: Caller = Caller { user: 1, device: 1, admin: true };

/// What the API's handlers share, over `db` in `temp`.
fn state(temp: &tempfile::TempDir, db: Arc<crate::db::Database>) -> AppState {
    let plugins = Plugins::new(db.clone(), temp.path().join("plugins"), HashMap::new());
    AppState {
        db,
        images: Arc::new(Images::new(temp.path().join("cache"), "ffmpeg".into())),
        indexes: Arc::default(),
        setup: Arc::default(),
        plugins: Arc::new(plugins),
        hub: Arc::default(),
        players: Arc::default(),
        prefix: "/api/v1".into(),
    }
}

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

/// Command ID `n`, as a UUID.
fn id(n: u8) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

#[tokio::test]
async fn a_revoked_connection_acts_for_no_one() {
    let (temp, db, _app) = app_with_tracks().await;
    let state = state(&temp, db);
    let phone = Caller { device: 2, ..OWNER };
    let mut other = state.players.connect(&state.hub, 1, phone.device);
    let revoked = state.players.connect(&state.hub, 1, OWNER.device);
    let progress = |start: bool| {
        let report = json!({
            "deviceId": "1", "itemId": "item-1", "libraryId": "1", "trackId": "1",
            "status": "playing", "positionMs": if start { 0 } else { 5000 }, "start": start,
        });
        json!({ "type": "progress", "report": report })
    };
    // The connection plays, and is sent a command from the phone to carry out.
    let play = progress(true).to_string();
    assert!(handle(&state, OWNER, &revoked, &play).await.is_none());
    let pause = json!({ "commandId": id(1), "type": "pause" });
    let pause = json!({ "type": "playerCommand", "command": pause }).to_string();
    let told = handle(&state, phone, &other, &pause).await;
    let pending = json!({ "commandId": id(1), "status": "pending" });
    assert_eq!(serde_json::to_value(told).unwrap()["result"], pending);
    queued(&mut other).await;
    let before = playback(&state).await;
    assert_eq!(before["activeDeviceId"], json!("1"));

    // The device's token is left in place, so only the closed connection stands in the way.
    state.hub.revoke(1, OWNER.device, Revoked::LoggedOut);
    let applied = json!({ "commandId": id(1), "status": "applied" });
    let transfer = json!({ "commandId": id(2), "type": "transfer", "deviceId": "2" });
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
