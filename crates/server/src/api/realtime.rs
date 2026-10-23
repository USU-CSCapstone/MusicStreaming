//! The realtime connection: `openRealtime` (`requirements/realtime.md`, `design/general.md` §5).
//!
//! It authenticates like any other request, by bearer token or session cookie. A browser sends
//! the cookie with a WebSocket opened by any page on the same site, sibling subdomains
//! included, and cannot be made to add a header such as `X-Jewelcase-Client`, so a connection
//! from a page is refused unless the page was served by this server.
//!
//! The server pings every [`HEARTBEAT`] and drops a connection it has heard nothing from for
//! [`SILENCE`], so a device that went away stops counting as connected.
//!
//! A device is authenticated only as it connects, so whatever signs one out must also
//! [`Hub::revoke`] it, which tells its connections and closes them.

mod hub;
mod messages;
mod origin;

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use rusqlite::OptionalExtension;
use tokio::time::{Instant, interval_at, timeout};

pub use hub::{Hub, Presence};
pub use messages::{Revoked, ServerMessage};

use super::authenticate::Caller;
use super::devices;
use super::player::{self, PlayerCommand};
use super::time::rfc3339;
use super::{AppState, Code, Id, Problem};
use crate::db::{Database, now_ms};
use messages::{ClientMessage, encode};
use origin::same_origin;

const HEARTBEAT: Duration = Duration::from_secs(15);

/// How long a connection may go without a word from its client, pongs included, before it is
/// dropped; also how long one send may take. Two missed heartbeats and then some.
const SILENCE: Duration = Duration::from_secs(40);

/// Client messages are commands and reports of a few hundred bytes. A bound well above that
/// keeps a client from making the server buffer the default 64 MiB.
const MAX_MESSAGE: usize = 64 * 1024;

/// `openRealtime`: upgrades to a WebSocket for the calling device.
pub async fn open(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, Problem> {
    if !same_origin(&headers) {
        return Err(Problem::new(Code::Forbidden)
            .detail("A realtime connection from a page must come from this server's own pages."));
    }
    let upgrade =
        upgrade.map_err(|_| Problem::invalid("GET /realtime must be a WebSocket upgrade"))?;
    Ok(upgrade
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| serve(socket, state, caller)))
}

/// Whether `device` is still signed in, and its account may still sign in.
async fn signed_in(db: &Database, device: i64) -> Result<bool, Problem> {
    let found = db.read(move |conn| {
        conn.query_row(
            "SELECT 1 FROM devices d JOIN users u ON u.id = d.user_id \
             WHERE d.id = ?1 AND d.revoked_reason IS NULL AND u.status = 'active'",
            [device],
            |_| Ok(()),
        )
        .optional()
    });
    Ok(found.await?.is_some())
}

/// Holds `socket` open for `caller` until either side closes it or it goes silent.
async fn serve(mut socket: WebSocket, state: AppState, caller: Caller) {
    // Joined only once upgraded, so every connection the hub counts is let go of below. A
    // sign-out between authenticating the request and joining would tell no one, so the device
    // is checked again once joined; any sign-out after that tells this connection.
    let mut presence = state.players.connect(&state.hub, caller.user, caller.device);
    let hello = ServerMessage::Hello { device_id: Id(caller.device), server_time: now() };
    let mut heard = Instant::now();
    if !matches!(signed_in(&state.db, caller.device).await, Ok(true)) {
        // Closed rather than dropped, so its client gives up at once and asks why.
        send(&mut socket, Message::Close(None)).await;
    } else if send(&mut socket, encode(&hello)).await {
        // Every connection of the account, this one included, learns it is connected.
        devices::changed(state.db.clone(), state.hub.clone(), caller.user);
        let mut heartbeat = interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
        loop {
            tokio::select! {
                received = socket.recv() => {
                    let Some(Ok(message)) = received else { break };
                    heard = Instant::now();
                    let Message::Text(text) = message else { continue };
                    let reply = handle(&state, caller, &presence, &text).await;
                    if let Some(reply) = reply && !send(&mut socket, encode(&reply)).await {
                        break;
                    }
                }
                message = presence.next() => {
                    let Some(message) = message else {
                        // The hub let go of this connection, after any last word it had for it.
                        send(&mut socket, Message::Close(None)).await;
                        break;
                    };
                    if !send(&mut socket, message).await {
                        break;
                    }
                }
                _ = heartbeat.tick() => {
                    if heard.elapsed() > SILENCE || !send(&mut socket, Message::Ping(Bytes::new())).await {
                        break;
                    }
                }
            }
        }
    }
    let connection = presence.id();
    drop(presence);
    // A connection that went silent is dropped up to [`SILENCE`] and a [`HEARTBEAT`] later, and
    // whatever was playing over it is taken to have got only as far as when it was last heard
    // from. One that ended any other way, even without a word, was there until it ended.
    let unheard = heard.elapsed();
    let unheard = if unheard > SILENCE { unheard.as_millis() } else { 0 };
    let heard_at = now_ms() - i64::try_from(unheard).unwrap_or(i64::MAX);
    state.players.disconnected(&state.hub, caller.user, connection, heard_at);
    // A connected device is in use the whole time, which requests alone would not show.
    let device = caller.device;
    let seen = state.db.write(move |tx| {
        tx.execute("UPDATE devices SET last_seen_at = ?2 WHERE id = ?1", [device, now_ms()])
    });
    if let Err(error) = seen.await {
        tracing::warn!(%error, "cannot record when a device was last seen");
    }
    devices::changed(state.db, state.hub, caller.user);
}

/// Acts on `text` from `caller`'s client, and answers the reply it gets, if any. Messages for
/// parts not built yet are ignored, as a newer client's would be.
async fn handle(
    state: &AppState,
    caller: Caller,
    presence: &Presence,
    text: &str,
) -> Option<ServerMessage> {
    let message = match serde_json::from_str(text) {
        Ok(message) => message,
        Err(error) => {
            tracing::debug!(%error, "realtime message ignored");
            return None;
        }
    };
    match message {
        ClientMessage::Ping => Some(ServerMessage::Pong { server_time: now() }),
        // A connection the hub has closed acts for no one: its device may be signed out.
        ClientMessage::Progress { .. }
        | ClientMessage::PlayerCommand { .. }
        | ClientMessage::CommandResult { .. }
            if !presence.is_open() =>
        {
            None
        }
        ClientMessage::Progress { report } => {
            if let Err(problem) = player::apply(state, caller, presence, report).await {
                tracing::debug!(?problem, "progress report refused");
                // So the device sees how things stand instead of what it reported.
                state.players.send_state(&state.hub, caller.user, presence);
            }
            None
        }
        ClientMessage::PlayerCommand { command } => {
            let command = match PlayerCommand::read(command) {
                Ok(command) => command,
                Err(result) => return result.map(|result| ServerMessage::CommandResult { result }),
            };
            let result =
                state.players.command_from_connection(&state.hub, caller, presence.id(), command);
            Some(ServerMessage::CommandResult { result })
        }
        ClientMessage::CommandResult { result } => {
            state.players.answered(&state.hub, caller.user, presence.id(), result);
            None
        }
    }
}

/// Sends `message`, and whether it went within [`SILENCE`].
async fn send(socket: &mut WebSocket, message: Message) -> bool {
    matches!(timeout(SILENCE, socket.send(message)).await, Ok(Ok(())))
}

fn now() -> String {
    rfc3339(now_ms())
}

#[cfg(test)]
mod tests;
