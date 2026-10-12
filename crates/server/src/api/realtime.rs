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

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, header};
use axum::response::Response;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, interval_at, timeout};

pub use hub::{Hub, Presence};

use super::authenticate::{Caller, unauthenticated};
use super::devices::{self, Device};
use super::player::{self, CommandResult, PlayerCommand, PlayerState, ProgressReport};
use super::time::rfc3339;
use super::{AppState, Code, Id, Problem};
use crate::db::{Database, now_ms};

const HEARTBEAT: Duration = Duration::from_secs(15);

/// How long a connection may go without a word from its client, pongs included, before it is
/// dropped; also how long one send may take. Two missed heartbeats and then some.
const SILENCE: Duration = Duration::from_secs(40);

/// Client messages are commands and reports of a few hundred bytes. A bound well above that
/// keeps a client from making the server buffer the default 64 MiB.
const MAX_MESSAGE: usize = 64 * 1024;

/// Set by proxies that change `Host` (`requirements/deployment.md` §6).
const FORWARDED_HOST: &str = "x-forwarded-host";

/// The spec's `RealtimeClientMessage`, as far as it is built.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ClientMessage {
    Ping,
    Progress {
        report: ProgressReport,
    },
    /// Read in two steps, so one that is not a command it carries out can still be answered.
    PlayerCommand {
        command: serde_json::Value,
    },
    CommandResult {
        result: CommandResult,
    },
}

/// The spec's `RealtimeServerMessage`, as far as it is built.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ServerMessage {
    Hello {
        device_id: Id,
        server_time: String,
    },
    Pong {
        server_time: String,
    },
    Devices {
        devices: Vec<Device>,
    },
    Player {
        state: PlayerState,
    },
    /// A command for the connection playing to carry out.
    Execute {
        command: PlayerCommand,
    },
    CommandResult {
        result: CommandResult,
    },
    SessionRevoked {
        reason: Revoked,
    },
}

/// Why a device was signed out: the spec's `sessionRevoked` `reason`.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Revoked {
    LoggedOut,
}

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
    // A sign-out between authenticating the request and joining the hub would tell no one, so
    // the device is checked again once joined. Any sign-out after that tells this connection.
    let presence = state.hub.connect(caller.user, caller.device);
    if !signed_in(&state.db, caller.device).await? {
        return Ok(unauthenticated());
    }
    Ok(upgrade
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| serve(socket, presence, state, caller)))
}

/// Whether `device` still has a token, and its account may still sign in.
async fn signed_in(db: &Database, device: i64) -> Result<bool, Problem> {
    let found = db.read(move |conn| {
        conn.query_row(
            "SELECT 1 FROM devices d JOIN users u ON u.id = d.user_id \
             WHERE d.id = ?1 AND d.token_hash IS NOT NULL AND u.status = 'active'",
            [device],
            |_| Ok(()),
        )
        .optional()
    });
    Ok(found.await?.is_some())
}

/// Whether the request is not from a page, which sends no `Origin`, or is from a page this
/// server served: one whose origin names the host the request was sent to.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else { return true };
    let Some((_, origin)) = origin.to_str().ok().and_then(|origin| origin.split_once("://")) else {
        return false;
    };
    // A proxy that lists several hosts puts the one the client asked for first.
    let host = |name| Some(headers.get(name)?.to_str().ok()?.split(',').next()?.trim());
    [host(header::HOST.as_str()), host(FORWARDED_HOST)]
        .into_iter()
        .flatten()
        .any(|host| host.eq_ignore_ascii_case(origin))
}

/// Holds `socket` open for `caller` until either side closes it or it goes silent.
async fn serve(mut socket: WebSocket, mut presence: Presence, state: AppState, caller: Caller) {
    let hello = ServerMessage::Hello { device_id: Id(caller.device), server_time: now() };
    let mut heard = Instant::now();
    if send(&mut socket, encode(&hello)).await {
        state.players.send_state(&state.hub, caller.user, &presence);
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
    // A connection that went silent is dropped up to a minute later, and whatever was playing
    // over it is taken to have got only as far as when it was last heard from.
    let heard_at = now_ms() - i64::try_from(heard.elapsed().as_millis()).unwrap_or(i64::MAX);
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
            // Authenticated as the connection opened, perhaps hours ago, so checked again.
            let signed_in = signed_in(&state.db, caller.device).await.unwrap_or(false);
            if !signed_in || !presence.is_open() {
                return None;
            }
            let result = state.players.command(&state.hub, caller, presence.id(), command);
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

fn encode(message: &ServerMessage) -> Message {
    Message::text(serde_json::to_string(message).expect("server messages serialize"))
}

fn now() -> String {
    rfc3339(now_ms())
}

#[cfg(test)]
mod tests;
