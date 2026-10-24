//! Playback for the player's and realtime tests: its messages, and reports over a socket.

use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::socket::{Socket, next_of};

/// Player command ID `n`, as the UUID a client would send.
pub fn command_id(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// A progress report that item `item-1`, track 1 of library 1, is `status` at 1000 ms, with the
/// fields of `changes` added or replacing those. Panics if `changes` is not an object.
pub fn progress_report(status: &str, changes: Value) -> Value {
    let mut report = json!({
        "itemId": "item-1", "libraryId": "1", "trackId": "1", "status": status, "positionMs": 1000,
    });
    report.as_object_mut().unwrap().extend(changes.as_object().unwrap().clone());
    report
}

/// Sends [`progress_report`] over `socket` and returns once the server has handled it: a
/// connection handles its messages in order, so that is when the ping sent after it is
/// answered. Skips whatever `socket` is sent before that answer.
pub async fn report(socket: &mut Socket, status: &str, changes: Value) {
    let progress = json!({ "type": "progress", "report": progress_report(status, changes) });
    socket.send(Message::text(progress.to_string())).await.unwrap();
    socket.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    next_of(socket, "pong").await;
}

/// Starts playback on `socket`, taking it over from any other connection, and returns once the
/// server has it.
pub async fn start_playing(socket: &mut Socket) {
    report(socket, "playing", json!({ "start": true })).await;
}
