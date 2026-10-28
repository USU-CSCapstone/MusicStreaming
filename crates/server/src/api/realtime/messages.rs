//! The messages a realtime connection carries, each way.

use axum::extract::ws::Message;
use serde::{Deserialize, Serialize};

use crate::api::devices::Device;
use crate::api::player::{CommandResult, PlayerCommand, PlayerState, ProgressReport};
use crate::api::{Id, Uuid};
use jewelcase_core::Edit;

/// The spec's `RealtimeClientMessage`, as far as it is built.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum ClientMessage {
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
    /// A command for the connection playing to carry out, or a `setVolume` or `transfer` for
    /// the device it names.
    Execute {
        command: PlayerCommand,
    },
    CommandResult {
        result: CommandResult,
    },
    /// An edit the server applied to the account's queue, as applied: an add's query is
    /// resolved into its tracks.
    QueueEdit {
        queue_id: Uuid,
        /// The queue's version once the edit is applied, one more than the last edit's.
        version: u32,
        edit_id: Uuid,
        edit: Edit,
    },
    /// The account's queue was replaced by a start, so a device fetches it again.
    QueueReplaced {
        queue_id: Uuid,
    },
    SessionRevoked {
        reason: Revoked,
    },
}

/// Why a device was signed out: the spec's `reason` in `sessionRevoked` and in a
/// `session_revoked` Problem, stored by the same name in `devices.revoked_reason`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Revoked {
    /// The account's credentials changed: the device keeps its downloads.
    CredentialsChanged,
    /// Signed out: the device removes its downloads.
    LoggedOut,
}

/// `message` as the text frame that carries it.
pub(super) fn encode(message: &ServerMessage) -> Message {
    Message::text(serde_json::to_string(message).expect("server messages serialize"))
}
