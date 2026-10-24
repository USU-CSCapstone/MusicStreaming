//! Remote control: player commands from any of an account's devices, carried out by the
//! connection playing, or by the device named for `setVolume` and `transfer`
//! (`requirements/realtime.md` §3, §5).
//!
//! A command arrives as `playerCommand` over a realtime connection, or over HTTP from a client
//! without one. The server passes it on to the playing connection as `execute`, which reports
//! a `commandResult`. Over a realtime connection the sender is answered `pending` at once and
//! the result follows; over HTTP the answer is the result. If none comes within a few seconds,
//! or the connection closes first, the command failed. One for whatever plays, as all but
//! `setVolume` and a transfer to another device are, also fails if playback starts on another
//! connection first. Commands reach the one playing connection in the order they arrive, so the
//! last wins (§7).
//!
//! The results of an account's latest commands are kept, so one sent again, by a client unsure
//! the first arrived, is carried out once and answered with how it went.
//!
//! Every command goes to the connection playing but `transfer` (Shift) and `setVolume`, which
//! go to the device they name. A transfer takes playback over from wherever it is, or from where
//! it stopped; so it also wakes a device that is idle (§3). The server does not keep the queue,
//! so that device picks up only the track and where it was in it; nothing after it comes along.
//! Volume belongs to the device, so `setVolume` reaches one that is not playing too.

mod relay;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;

use super::Players;
use crate::api::realtime::Hub;
use crate::api::scope::Caller;
use crate::api::{AnyId, Problem, Uuid, extract};
pub use relay::Commands;
use relay::Reply;

/// The spec's `PlayerCommand`.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerCommand {
    command_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<AnyId>,
    #[serde(flatten)]
    action: Action,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum Action {
    Play,
    Pause,
    Next,
    Previous,
    Seek {
        position_ms: u32,
    },
    SetVolume {
        volume: f64,
    },
    Transfer {
        /// Whether it plays once it arrives, or arrives paused.
        #[serde(default = "yes")]
        play: bool,
    },
}

/// The default for `Transfer`'s `play`: a transfer plays unless it says not to.
fn yes() -> bool {
    true
}

/// The spec's `CommandResult`.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    command_id: String,
    status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Pending,
    Applied,
    Failed,
}

impl CommandResult {
    /// Command `command_id`'s result, with `reason` if there is one to give.
    fn new(command_id: String, status: Status, reason: Option<&str>) -> CommandResult {
        CommandResult { command_id, status, reason: reason.map(str::to_owned) }
    }
}

impl PlayerCommand {
    /// Reads `value` as a command, or answers why it cannot be carried out if it names itself
    /// with a `commandId`, so its sender is not left believing it was (§5).
    pub fn read(value: Value) -> Result<PlayerCommand, Option<CommandResult>> {
        // In lowercase, as `Uuid` reads an ID, so it matches every other result's.
        let id = value.get("commandId").and_then(Value::as_str).map(str::to_ascii_lowercase);
        serde_json::from_value(value).map_err(|_| {
            let reason = Some("Not a command this server carries out.");
            id.map(|id| CommandResult::new(id, Status::Failed, reason))
        })
    }
}

impl Players {
    /// [`command`](Self::command) from `caller`'s realtime connection `connection`: what to
    /// tell it now, which is `failed` if the command was not passed on.
    pub fn command_from_connection(
        self: &Arc<Self>,
        hub: &Arc<Hub>,
        caller: Caller,
        connection: u64,
        command: PlayerCommand,
    ) -> CommandResult {
        let id = command.command_id.as_str().to_owned();
        let reply = Reply::Connection(connection);
        self.command(hub, caller, reply, command)
            .unwrap_or_else(|refused| CommandResult::new(id, Status::Failed, refused.explanation()))
    }
}

/// `sendPlayerCommand`: answers how the device carried the command out, once it has.
pub async fn send(
    State(players): State<Arc<Players>>,
    State(hub): State<Arc<Hub>>,
    caller: Caller,
    extract::Json(command): extract::Json<PlayerCommand>,
) -> Result<Json<CommandResult>, Problem> {
    let (reply, answer) = oneshot::channel();
    let result = players.command(&hub, caller, Reply::Request(reply), command)?;
    // Answered here once it has finished, unless the command is sent again meanwhile: its
    // result then goes where it was sent from last, and this request answers `pending`.
    if result.status == Status::Pending
        && let Ok(answer) = answer.await
    {
        return Ok(Json(answer));
    }
    Ok(Json(result))
}

#[cfg(test)]
mod tests;
