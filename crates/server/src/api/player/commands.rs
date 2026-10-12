//! Remote control: player commands from any of an account's connections, carried out by the
//! one playing (`requirements/realtime.md` §3, §5).
//!
//! The server passes a command on to the playing connection as `execute` and answers the
//! sender `pending` at once. The playing connection reports a `commandResult`, which goes back
//! to the sender; if none comes within [`ANSWER_WITHIN`], the sender is told the command
//! failed. Commands reach the one playing connection in the order they arrive, so the last
//! wins (§7).
//!
//! Waking an idle device and moving playback to another (Shift) need the queue, so for now
//! every command goes to the device playing, and one that names another device fails.

use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::AbortHandle;

use super::Players;
use crate::api::Id;
use crate::api::authenticate::Caller;
use crate::api::plays::is_uuid;
use crate::api::realtime::{Hub, ServerMessage};

/// How long the playing connection has to answer a command before it counts as failed.
const ANSWER_WITHIN: Duration =
    if cfg!(test) { Duration::from_secs(2) } else { Duration::from_secs(5) };

/// How many of an account's commands may wait for an answer at once, so a flood of them
/// cannot fill the playing connection's outbox and close it.
const MAX_WAITING: usize = 16;

/// Longer reasons from the playing connection are cut short.
const MAX_REASON: usize = 200;

/// The spec's `PlayerCommand`, but for `transfer`, which needs the queue.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerCommand {
    command_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<String>,
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
    Seek { position_ms: u32 },
    SetVolume { volume: f64 },
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

/// A command passed on and not yet answered, under its account and `commandId`: IDs are the
/// clients' own, so two accounts may choose the same one.
pub type Key = (i64, String);

pub struct Waiting {
    /// The connection to tell how it went: where it was last sent from.
    from: u64,
    /// The connection it went to, the only one whose answer counts.
    to: u64,
    /// Fails it if no answer comes.
    timer: AbortHandle,
}

impl CommandResult {
    fn new(command_id: String, status: Status, reason: Option<&str>) -> CommandResult {
        CommandResult { command_id, status, reason: reason.map(str::to_owned) }
    }
}

impl PlayerCommand {
    /// Reads `value` as a command, or answers why it cannot be carried out if it names itself
    /// with a `commandId`, so its sender is not left believing it was (§5).
    pub fn read(value: Value) -> Result<PlayerCommand, Option<CommandResult>> {
        let id = value.get("commandId").and_then(Value::as_str).map(str::to_owned);
        serde_json::from_value(value).map_err(|_| {
            let reason = Some("Not a command this server carries out.");
            id.map(|id| CommandResult::new(id, Status::Failed, reason))
        })
    }
}

impl Players {
    /// Passes `command` from `caller`'s connection `from` on to the connection playing, and
    /// answers what to tell the sender now: `pending`, or why it failed.
    pub fn command(
        self: &Arc<Self>,
        hub: &Arc<Hub>,
        caller: Caller,
        from: u64,
        command: PlayerCommand,
    ) -> CommandResult {
        let failed =
            |reason| CommandResult::new(command.command_id.clone(), Status::Failed, Some(reason));
        if !is_uuid(&command.command_id) {
            return failed("commandId must be a UUID.");
        }
        if let Action::SetVolume { volume } = command.action {
            if command.device_id.is_none() {
                return failed("setVolume must name its deviceId: volume belongs to a device.");
            }
            if !(0.0..=1.0).contains(&volume) {
                return failed("volume must be from 0 to 1.");
            }
        }
        let accounts = self.lock();
        let playing = accounts
            .get(&caller.user)
            .and_then(|state| Some((state.active_connection?, state.active_device_id?)));
        let Some((to, device)) = playing else {
            return failed("Nothing is playing.");
        };
        if command.device_id.as_deref().is_some_and(|id| Id::canonical(id) != Some(device)) {
            return failed("That device is not the one playing.");
        }
        let key = (caller.user, command.command_id.clone());
        let pending = CommandResult::new(key.1.clone(), Status::Pending, None);
        let mut waiting = self.waiting.lock().expect("player commands lock");
        // Sent again, as by a client unsure the first arrived, perhaps from a new connection:
        // it is passed on once, and how it went goes to where it was last sent from.
        if let Some(earlier) = waiting.get_mut(&key) {
            earlier.from = from;
            return pending;
        }
        if waiting.keys().filter(|(user, _)| *user == caller.user).count() >= MAX_WAITING {
            return failed("Too many commands are waiting for the device playing.");
        }
        hub.send(caller.user, to, &ServerMessage::Execute { command });
        let (players, timer_hub, timer_key) = (self.clone(), hub.clone(), key.clone());
        let timer = tokio::spawn(async move {
            tokio::time::sleep(ANSWER_WITHIN).await;
            players.fail(&timer_hub, &timer_key, "The device playing did not answer.");
        });
        waiting.insert(key, Waiting { from, to, timer: timer.abort_handle() });
        pending
    }

    /// Passes `result`, from `user`'s connection `connection`, back to the connection whose
    /// command it answers, if it is the connection that command went to.
    pub fn answered(&self, hub: &Hub, user: i64, connection: u64, mut result: CommandResult) {
        if result.status == Status::Pending {
            return;
        }
        let mut waiting = self.waiting.lock().expect("player commands lock");
        let Entry::Occupied(entry) = waiting.entry((user, result.command_id.clone())) else {
            return;
        };
        if entry.get().to != connection {
            return;
        }
        let command = entry.remove();
        drop(waiting);
        command.timer.abort();
        result.reason = result.reason.map(|reason| reason.chars().take(MAX_REASON).collect());
        hub.send(user, command.from, &ServerMessage::CommandResult { result });
    }

    /// Fails at once every command waiting on `user`'s connection `connection`, now closed.
    pub(super) fn fail_commands_to(&self, hub: &Hub, user: i64, connection: u64) {
        let gone: Vec<_> = {
            let mut waiting = self.waiting.lock().expect("player commands lock");
            let to_it =
                |key: &Key, command: &mut Waiting| key.0 == user && command.to == connection;
            waiting.extract_if(to_it).collect()
        };
        for ((user, command_id), command) in gone {
            command.timer.abort();
            let reason = Some("The device playing went away.");
            let result = CommandResult::new(command_id, Status::Failed, reason);
            hub.send(user, command.from, &ServerMessage::CommandResult { result });
        }
    }

    /// Tells the sender of the command `key`, if it still waits, that it failed for `reason`.
    fn fail(&self, hub: &Hub, key: &Key, reason: &str) {
        let Some(command) = self.waiting.lock().expect("player commands lock").remove(key) else {
            return;
        };
        let result = CommandResult::new(key.1.clone(), Status::Failed, Some(reason));
        hub.send(key.0, command.from, &ServerMessage::CommandResult { result });
    }
}

#[cfg(test)]
mod tests;
