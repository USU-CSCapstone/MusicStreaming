//! Playback for the player's and realtime tests: its messages, and reports over a socket.

use axum::Router;
use axum::http::StatusCode;
use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::call;
use super::socket::{Socket, next_of};

/// The key of the source [`start_queue`] starts.
const KEY: &str = "0b1b1b1b-3b7e-4d43-9a8e-0d1c2b3a4f5e";

/// Item `n` of the queue [`start_queue`] starts: track `n + 1` of library 1.
pub fn item(n: usize) -> String {
    format!("{KEY}.{n}")
}

/// Starts a queue of library 1's tracks 1 and 2 for the account `token` belongs to, so its
/// reports have items to name: [`item`] 0 and 1.
pub async fn start_queue(app: &Router, token: &str) {
    let queue = json!({
        "queueId": "5f0c6a52-3b7e-4d43-9a8e-0d1c2b3a4f5e", "context": { "type": "album" },
        "start": 0, "source": { "key": KEY, "libraryId": "1", "tracks": ["1", "2"] },
    });
    let (status, _) = call(app, token, "POST", "/api/v1/me/queue", Some(queue)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// Player command ID `n`, as the UUID a client would send.
pub fn command_id(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// A progress report that [`item`] 0 is `status` at 1000 ms, with the fields of `changes` added
/// or replacing those. Panics if `changes` is not an object.
pub fn progress_report(status: &str, changes: Value) -> Value {
    let mut report = json!({
        "itemId": item(0), "status": status, "positionMs": 1000,
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
