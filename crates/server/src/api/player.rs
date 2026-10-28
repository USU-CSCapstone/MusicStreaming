//! Playback state: `getPlayer`, and the `progress` reports that change it
//! (`requirements/playback.md` §6, `requirements/realtime.md` §2, §4).
//!
//! The server keeps each account's state in memory and sends it to all of the account's
//! realtime connections whenever a report changes it. The playing device reports its progress
//! over its own connection, which it needs anyway to be controlled and to hear it was taken
//! over from.
//! A report marked `start` comes from a device the user just started playback on, and takes
//! over from whichever device was playing; that device learns it from the state it is sent and
//! stops (§2). Any other report from a connection that is not the active one is refused, so the
//! device that was taken over from cannot take playback back with reports it sent before it
//! heard. A refused report is answered with the current state.
//!
//! A report names its place in the account's queue by `itemId`. The server accepts only items
//! the queue has, in any of its sections; a track is found through the queue, and a device holds
//! the same queue, so the state needs no track of its own. A new queue replaces the old without
//! touching the state, which keeps naming its item until the next report: a client that finds
//! the item absent from its queue has nothing to resume or hand over.
//!
//! Position is timed by when a report arrives: a device's clock may be off by more than a
//! report takes to arrive.
//!
//! Where each account was is also saved, to resume from after the server restarts.

mod commands;
mod progress;
mod saved;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Json;
use axum::extract::State;
use serde::Serialize;

use super::Id;
use super::realtime::{Hub, Presence, ServerMessage};
use super::scope::Caller;
use super::time::serialize_rfc3339;
use crate::db::now_ms;
pub use commands::{CommandResult, PlayerCommand, send as send_command};
use jewelcase_core::Recover;
pub use progress::{ProgressReport, apply};

/// Each account's playback state, for those that have played anything.
#[derive(Default)]
pub struct Players {
    accounts: Mutex<HashMap<i64, PlayerState>>,
    /// Each account's remote commands, waiting and lately answered.
    commands: Mutex<HashMap<i64, commands::Commands>>,
}

/// The spec's `PlayerState`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    status: Status,
    /// The device playing or paused; `None` when stopped (`requirements/realtime.md` §2).
    active_device_id: Option<Id>,
    /// Which of that device's connections plays: a device may hold several, such as two
    /// browser tabs, and only one of them may report.
    #[serde(skip)]
    active_connection: Option<u64>,
    /// The queue item playback is at, as the queue names it.
    item_id: Option<String>,
    position_ms: u32,
    /// When `position_ms` was true, in server time.
    #[serde(serialize_with = "serialize_rfc3339")]
    position_at: i64,
    volume: Option<f64>,
    /// Whether it changed since it was last saved.
    #[serde(skip)]
    unsaved: bool,
    /// The item and position it was last saved at, and when.
    #[serde(skip)]
    saved: Option<(String, u32, std::time::Instant)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Playing,
    Paused,
    Stopped,
}

impl PlayerState {
    /// The state of an account that has not played anything.
    fn stopped() -> PlayerState {
        PlayerState {
            status: Status::Stopped,
            active_device_id: None,
            active_connection: None,
            item_id: None,
            position_ms: 0,
            position_at: now_ms(),
            volume: None,
            unsaved: false,
            saved: None,
        }
    }

    /// Where playback is at `now`: past where it was reported, if it is playing. A clock that
    /// stepped back counts as no time.
    fn played_to(&self, now: i64) -> u32 {
        let played = match self.status {
            Status::Playing => u32::try_from((now - self.position_at).max(0)).unwrap_or(u32::MAX),
            Status::Paused | Status::Stopped => 0,
        };
        self.position_ms.saturating_add(played)
    }
}

impl Players {
    /// `user`'s current state.
    fn current(&self, user: i64) -> PlayerState {
        self.lock().get(&user).cloned().unwrap_or_else(PlayerState::stopped)
    }

    /// Counts `device` of `user` as connected, with its account's current state queued first.
    /// Queued under the lock a transfer holds to pick its connection, so no transfer reaches the
    /// new connection before the state does.
    pub fn connect(&self, hub: &Arc<Hub>, user: i64, device: i64) -> Presence {
        let accounts = self.lock();
        let presence = hub.connect(user, device);
        let state = accounts.get(&user).cloned().unwrap_or_else(PlayerState::stopped);
        hub.send(user, presence.id(), &ServerMessage::Player { state });
        presence
    }

    /// Sends one connection its account's current state when a report of its is refused. Sent
    /// in turn with changes, so it cannot arrive after a newer state.
    pub fn send_state(&self, hub: &Hub, user: i64, presence: &Presence) {
        let accounts = self.lock();
        let state = accounts.get(&user).cloned().unwrap_or_else(PlayerState::stopped);
        hub.send(user, presence.id(), &ServerMessage::Player { state });
    }

    /// Stops `user`'s playback where it got to by `heard_at`, when its client was last heard
    /// from, if `connection`, now closed, was the one playing, and tells the account's other
    /// connections (`requirements/realtime.md` §6). Any of them can carry on from there.
    pub fn disconnected(&self, hub: &Hub, user: i64, connection: u64, heard_at: i64) {
        let mut accounts = self.lock();
        // Under the lock, so no command can see this connection still playing and wait on it.
        self.fail_commands_to(hub, user, connection);
        let Some(state) = accounts.get_mut(&user) else { return };
        if state.active_connection != Some(connection) {
            return;
        }
        state.position_ms = state.played_to(heard_at);
        state.position_at = now_ms();
        state.status = Status::Stopped;
        state.active_device_id = None;
        state.active_connection = None;
        state.unsaved = true;
        hub.broadcast(user, &ServerMessage::Player { state: state.clone() });
    }

    /// Each account's playback state, locked.
    fn lock(&self) -> MutexGuard<'_, HashMap<i64, PlayerState>> {
        self.accounts.lock().recover()
    }
}

/// `getPlayer`.
pub async fn get(State(players): State<Arc<Players>>, caller: Caller) -> Json<PlayerState> {
    Json(players.current(caller.user))
}

#[cfg(test)]
mod tests;
