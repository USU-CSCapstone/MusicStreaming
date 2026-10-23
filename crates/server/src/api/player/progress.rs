//! The `progress` reports from the device playing, checked and applied.

use rusqlite::params;
use serde::Deserialize;

use super::{PlayerState, Players, Status};
use crate::api::realtime::{Hub, Presence, ServerMessage};
use crate::api::scope::Caller;
use crate::api::session::device_signed_in;
use crate::api::{AnyId, AppState, Id, Problem};
use crate::db::{now_ms, reaches};

/// Longer item IDs are refused, so no one keeps megabytes in memory as their place in a queue.
const MAX_ITEM_ID: usize = 100;

/// The spec's `ProgressReport`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressReport {
    device_id: AnyId,
    item_id: String,
    library_id: AnyId,
    track_id: AnyId,
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

impl Players {
    /// Applies `report` from `caller` over `connection`, already checked, and sends the new
    /// state to the account's connections. Refuses a report from any connection but the active
    /// one unless it starts playback; one that starts fails the commands waiting on another
    /// connection that was playing, but leaves a transfer to another device to finish.
    pub(super) fn report(
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
        if starts {
            self.fail_commands_taken_over_by(hub, user, connection);
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
}

/// Checks `report` from `caller` over the connection `presence` and applies it, unless the hub
/// closed the connection while it was checked, as when the server stops: a closed connection
/// acts for no one, so the report is dropped.
pub async fn apply(
    state: &AppState,
    caller: Caller,
    presence: &Presence,
    report: ProgressReport,
) -> Result<(), Problem> {
    if report.device_id.0 != Some(caller.device) {
        return Err(Problem::invalid("deviceId must be the reporting device's."));
    }
    if report.item_id.is_empty() || report.item_id.chars().count() > MAX_ITEM_ID {
        return Err(Problem::invalid(format!("itemId must be 1 to {MAX_ITEM_ID} characters.")));
    }
    if report.volume.is_some_and(|volume| !(0.0..=1.0).contains(&volume)) {
        return Err(Problem::invalid("volume must be from 0 to 1."));
    }
    // A malformed ID, like one out of reach, is not found, exactly as a track that does not
    // exist (`requirements/users.md` §10).
    let (AnyId(Some(library)), AnyId(Some(track))) = (report.library_id, report.track_id) else {
        return Err(Problem::not_found());
    };
    // The connection was authenticated as it opened, perhaps hours ago, so whether the device
    // is still signed in, its account still active, and the library still in its reach are
    // read again.
    let reachable = state.db.read(move |conn| {
        conn.prepare_cached(concat!(
            "SELECT EXISTS (SELECT 1 FROM devices d JOIN users u ON u.id = d.user_id \
             JOIN tracks t ON t.id = ?2 AND t.library_id = ?1 \
             WHERE d.id = ?3 AND ",
            device_signed_in!(),
            " AND ",
            reaches!(admin: "u.role <> 'user'", user: "u.id", library: "?1"),
            ")"
        ))?
        .query_row(params![library, track, caller.device], |row| row.get(0))
    });
    if !reachable.await? {
        return Err(Problem::not_found());
    }
    if !presence.is_open() {
        return Ok(());
    }
    state.players.report(&state.hub, caller, presence.id(), report, Id(library), Id(track))
}
