//! The `progress` reports from the device playing, checked and applied.

use serde::Deserialize;

use super::{PlayerState, Players, Status};
use crate::api::realtime::{Hub, Presence, ServerMessage};
use crate::api::scope::Caller;
use crate::api::session::device_signed_in;
use crate::api::{AppState, Id, Problem};
use crate::db::now_ms;

/// The spec's `ProgressReport`. A report is the device's whose connection sends it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressReport {
    item_id: String,
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
    /// one unless it starts playback.
    pub(super) fn report(
        &self,
        hub: &Hub,
        caller: Caller,
        connection: u64,
        report: ProgressReport,
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
    if report.volume.is_some_and(|volume| !(0.0..=1.0).contains(&volume)) {
        return Err(Problem::invalid("volume must be from 0 to 1."));
    }
    // An item the queue does not have, which an account that has no queue never does, is not
    // found. The item's track was checked against the account's reach as it joined the queue,
    // and the state names only the item, whose track a client looks up where it is checked
    // again, so a report needs no check of reach of its own.
    if !state.queues.holds(caller.user, &report.item_id) {
        return Err(Problem::not_found());
    }
    // The connection was authenticated as it opened, perhaps hours ago, so whether the device
    // is still signed in and its account still active are read again.
    let signed_in = state.db.read(move |conn| {
        conn.prepare_cached(concat!(
            "SELECT EXISTS (SELECT 1 FROM devices d JOIN users u ON u.id = d.user_id \
             WHERE d.id = ?1 AND ",
            device_signed_in!(),
            ")"
        ))?
        .query_row([caller.device], |row| row.get(0))
    });
    if !signed_in.await? {
        return Err(Problem::not_found());
    }
    if !presence.is_open() {
        return Ok(());
    }
    state.players.report(&state.hub, caller, presence.id(), report)
}
