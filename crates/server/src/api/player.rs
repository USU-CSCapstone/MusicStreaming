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
//! Until the server keeps the queue, a report names its track itself, and `itemId` is the
//! device's own name for its place in its queue.
//!
//! Position is timed by when a report arrives: a device's clock may be off by more than a
//! report takes to arrive.
//!
//! Where each account was is also saved, to resume from after the server restarts.

mod commands;
mod saved;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Json;
use axum::extract::State;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::authenticate::Caller;
use super::realtime::{Hub, Presence, ServerMessage};
use super::time::serialize_rfc3339;
use super::{AppState, Id, Problem};
use crate::db::now_ms;
pub use commands::{CommandResult, PlayerCommand, send as send_command};

/// Longer item IDs are refused, so no one keeps megabytes in memory as their place in a queue.
const MAX_ITEM_ID: usize = 100;

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
    item_id: Option<String>,
    library_id: Option<Id>,
    track_id: Option<Id>,
    position_ms: u32,
    /// When `position_ms` was true, in server time.
    #[serde(serialize_with = "serialize_rfc3339")]
    position_at: i64,
    volume: Option<f64>,
    /// Whether it changed since it was last saved.
    #[serde(skip)]
    unsaved: bool,
    /// The item, track and position it was last saved at, and when.
    #[serde(skip)]
    saved: Option<(String, Id, u32, std::time::Instant)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Playing,
    Paused,
    Stopped,
}

/// The spec's `ProgressReport`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressReport {
    device_id: String,
    item_id: String,
    library_id: String,
    track_id: String,
    status: Reported,
    position_ms: u32,
    volume: Option<f64>,
    #[serde(default)]
    start: bool,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Reported {
    Playing,
    Paused,
    Stopped,
    Buffering,
}

impl PlayerState {
    /// The state of an account that has not played anything.
    fn stopped() -> PlayerState {
        PlayerState {
            status: Status::Stopped,
            active_device_id: None,
            active_connection: None,
            item_id: None,
            library_id: None,
            track_id: None,
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
    /// Under the lock a transfer picks its connection under, so none can arrive before it.
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

    /// Applies `report` from `caller` over `connection`, already checked, and sends the new
    /// state to the account's connections. Refuses a report from any connection but the active
    /// one unless it starts playback.
    fn report(
        &self,
        hub: &Hub,
        caller: Caller,
        connection: u64,
        report: ProgressReport,
        library: Id,
        track: Id,
    ) -> Result<(), Problem> {
        let (user, device) = (caller.user, caller.device);
        let mut accounts = self.lock();
        let starts = report.start && !matches!(report.status, Reported::Stopped);
        let active = accounts.get(&user).and_then(|state| state.active_connection);
        if !starts && active != Some(connection) {
            return Err(Problem::invalid("Only the playing connection reports, unless it starts."));
        }
        let state = accounts.entry(user).or_insert_with(PlayerState::stopped);
        // Nothing advances while buffering, so other devices are not shown running ahead.
        let status = match report.status {
            Reported::Playing => Status::Playing,
            Reported::Paused | Reported::Buffering => Status::Paused,
            Reported::Stopped => Status::Stopped,
        };
        // Volume belongs to the device (`requirements/realtime.md` §3): a report that leaves it
        // out keeps the active device's, but a device taking over does not inherit the last's.
        if report.volume.is_some() || state.active_device_id != Some(Id(device)) {
            state.volume = report.volume;
        }
        state.status = status;
        let playing = status != Status::Stopped;
        state.active_device_id = playing.then_some(Id(device));
        state.active_connection = playing.then_some(connection);
        state.item_id = Some(report.item_id);
        state.library_id = Some(library);
        state.track_id = Some(track);
        state.position_ms = report.position_ms;
        state.position_at = now_ms();
        state.unsaved = true;
        hub.broadcast(user, &ServerMessage::Player { state: state.clone() });
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<i64, PlayerState>> {
        self.accounts.lock().expect("player lock")
    }
}

/// Checks `report` from `caller` over the connection `presence` and applies it.
pub async fn apply(
    state: &AppState,
    caller: Caller,
    presence: &Presence,
    report: ProgressReport,
) -> Result<(), Problem> {
    if Id::canonical(&report.device_id) != Some(Id(caller.device)) {
        return Err(Problem::invalid("deviceId must be the reporting device's."));
    }
    if report.item_id.is_empty() || report.item_id.chars().count() > MAX_ITEM_ID {
        return Err(Problem::invalid(format!("itemId must be 1 to {MAX_ITEM_ID} characters.")));
    }
    if report.volume.is_some_and(|volume| !(0.0..=1.0).contains(&volume)) {
        return Err(Problem::invalid("volume must be from 0 to 1."));
    }
    // Out of reach is not found, exactly as a track that does not exist
    // (`requirements/users.md` §10).
    let (Some(library), Some(track)) =
        (Id::canonical(&report.library_id), Id::canonical(&report.track_id))
    else {
        return Err(Problem::not_found());
    };
    // The connection was authenticated as it opened, perhaps hours ago, so whether the device
    // is still signed in, its account still active, and the library still in its reach are
    // read again.
    let reachable = state.db.read(move |conn| {
        conn.prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM devices d JOIN users u ON u.id = d.user_id \
             JOIN tracks t ON t.id = ?2 AND t.library_id = ?1 \
             WHERE d.id = ?3 AND d.token_hash IS NOT NULL AND u.status = 'active' AND \
             (u.role <> 'user' OR EXISTS \
              (SELECT 1 FROM library_access WHERE user_id = u.id AND library_id = ?1)))",
        )?
        .query_row(params![library.0, track.0, caller.device], |row| row.get(0))
    });
    if !reachable.await? {
        return Err(Problem::not_found());
    }
    state.players.report(&state.hub, caller, presence.id(), report, library, track)
}

/// `getPlayer`.
pub async fn get(State(players): State<Arc<Players>>, caller: Caller) -> Json<PlayerState> {
    Json(players.current(caller.user))
}

#[cfg(test)]
mod tests;
