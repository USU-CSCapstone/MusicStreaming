//! Passing commands on to the connection that carries them out, and their results back to
//! their senders.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::task::AbortHandle;

use super::{Action, CommandResult, PlayerCommand, Status};
use crate::api::player::{Players, Status as PlayStatus};
use crate::api::realtime::{Hub, ServerMessage};
use crate::api::scope::Caller;
use crate::api::{AnyId, Code, Id, Problem};
use jewelcase_core::Recover;

/// How long a connection has to answer a command before it counts as failed, in milliseconds.
const ANSWER_WITHIN_MS: u16 = 5_000;

/// [`ANSWER_WITHIN_MS`] as a duration.
pub(super) const ANSWER_WITHIN: Duration = Duration::from_millis(ANSWER_WITHIN_MS as u64);

/// How many of an account's commands may wait for an answer at once, so a flood of them
/// cannot fill the outbox of the connection they go to and close it.
pub(super) const MAX_WAITING: usize = 16;

/// How many of an account's latest results are kept to answer a command sent again.
const REMEMBERED: usize = 32;

/// Longer reasons from the connection answering are cut short.
const MAX_REASON: usize = 200;

/// One account's commands, by their `commandId`: IDs are the clients' own, so two accounts
/// may choose the same one.
#[derive(Default)]
pub struct Commands {
    waiting: HashMap<String, Waiting>,
    /// The latest results, oldest first.
    finished: VecDeque<CommandResult>,
}

/// A command passed on and not yet answered.
struct Waiting {
    /// Where to say how it went: where it was last sent from.
    reply: Reply,
    /// The connection it went to, the only one whose answer counts.
    to: u64,
    /// Whether it went to whatever connection plays, as all but `setVolume` and a transfer to
    /// another device do, which go to the device they name.
    for_playback: bool,
    /// Fails it if no answer comes.
    timer: AbortHandle,
}

/// Where a command's result goes.
pub enum Reply {
    /// A realtime connection, by its ID.
    Connection(u64),
    /// An HTTP request waiting for it.
    Request(oneshot::Sender<CommandResult>),
}

impl Players {
    /// Passes `command` from `caller` on to the connection that carries it out, and answers what
    /// to tell the sender now: `pending`, `applied` for a transfer to the device already
    /// playing, or the result of a command sent again that has finished. Refuses a command that
    /// is invalid, has no connected device to go to, or would pass [`MAX_WAITING`]. A
    /// `setVolume` goes to the device it names, playing or not, since volume belongs to the
    /// device (`requirements/realtime.md` §3): to the connection playing if it is that device's,
    /// and otherwise to the device's newest.
    pub(super) fn command(
        self: &Arc<Self>,
        hub: &Arc<Hub>,
        caller: Caller,
        reply: Reply,
        mut command: PlayerCommand,
    ) -> Result<CommandResult, Problem> {
        if let Action::SetVolume { volume } = command.action {
            if command.device_id.is_none() {
                let reason = "setVolume must name its deviceId: volume belongs to a device.";
                return Err(Problem::invalid(reason));
            }
            if !(0.0..=1.0).contains(&volume) {
                return Err(Problem::invalid("volume must be from 0 to 1."));
            }
        }
        let id = command.command_id.as_str().to_owned();
        let pending = CommandResult::new(id.clone(), Status::Pending, None);
        let accounts = self.lock();
        let mut all = self.commands.lock().recover();
        let commands = all.entry(caller.user).or_default();
        // Sent again, as by a client unsure the first arrived, perhaps from a new connection:
        // it is carried out once, and how it went goes to where it was sent from last.
        if let Some(result) = commands.finished.iter().find(|result| result.command_id == id) {
            return Ok(result.clone());
        }
        if let Some(earlier) = commands.waiting.get_mut(&id) {
            earlier.reply = reply;
            return Ok(pending);
        }
        let state = accounts.get(&caller.user);
        let (to, for_playback) = if let Action::Transfer { .. } = command.action {
            let Some(AnyId(device)) = command.device_id else {
                return Err(Problem::invalid("transfer must name its deviceId."));
            };
            if state.and_then(|state| state.track_id).is_none() {
                return Err(unavailable("Nothing has played to move."));
            }
            let device = device.map(Id);
            let active = state.filter(|state| device.is_some() && state.active_device_id == device);
            if let Some(active) = active {
                // Moved where it already is: done, unless it is to play there and is paused,
                // which is as good as asking that connection to play.
                let play = matches!(command.action, Action::Transfer { play: true });
                match active.active_connection {
                    Some(to) if play && active.status != PlayStatus::Playing => {
                        command.action = Action::Play;
                        (to, true)
                    }
                    _ => return Ok(CommandResult::new(id, Status::Applied, None)),
                }
            } else {
                // A device of another account is as not connected as one can be.
                let newest = device.and_then(|device| hub.newest(caller.user, device.0));
                let Some(to) = newest else {
                    return Err(unavailable("That device is not connected."));
                };
                (to, false)
            }
        } else if let Action::SetVolume { .. } = command.action {
            let device = command.device_id.and_then(|AnyId(id)| id).map(Id);
            let playing = state
                .filter(|state| device.is_some() && state.active_device_id == device)
                .and_then(|state| state.active_connection);
            let newest = || device.and_then(|device| hub.newest(caller.user, device.0));
            let Some(to) = playing.or_else(newest) else {
                return Err(unavailable("That device is not connected."));
            };
            (to, false)
        } else {
            let playing =
                state.and_then(|state| Some((state.active_connection?, state.active_device_id?)));
            let Some((to, device)) = playing else {
                return Err(unavailable("Nothing is playing."));
            };
            if command.device_id.is_some_and(|AnyId(id)| id.map(Id) != Some(device)) {
                return Err(unavailable("That device is not the one playing."));
            }
            (to, true)
        };
        if commands.waiting.len() >= MAX_WAITING {
            // By then the oldest waiting command has been answered or has failed.
            let wait = i64::from(ANSWER_WITHIN_MS);
            let reason = "Too many commands are waiting for a device to answer.";
            return Err(Problem::rate_limited(wait).detail(reason));
        }
        // Sent while holding `accounts`: if sending closes the connection it goes to, its
        // `disconnected` waits for that lock, finds this command waiting, and fails it at once.
        hub.send(caller.user, to, &ServerMessage::Execute { command });
        let (players, timer_hub, user, timer_id) =
            (self.clone(), hub.clone(), caller.user, id.clone());
        let timer = tokio::spawn(async move {
            tokio::time::sleep(ANSWER_WITHIN).await;
            players.fail(&timer_hub, user, &timer_id, "The device did not answer.");
        });
        commands
            .waiting
            .insert(id, Waiting { reply, to, for_playback, timer: timer.abort_handle() });
        Ok(pending)
    }

    /// Passes `result`, from `user`'s connection `connection`, back to the sender of the
    /// command it answers, if it is the connection that command went to. A `pending` answer is
    /// ignored.
    pub fn answered(&self, hub: &Hub, user: i64, connection: u64, mut result: CommandResult) {
        if result.status == Status::Pending {
            return;
        }
        result.reason = result.reason.map(|reason| reason.chars().take(MAX_REASON).collect());
        // Commands are kept by their IDs in lowercase, as `Uuid` reads them, so an answer in
        // capitals finds its command too.
        result.command_id.make_ascii_lowercase();
        let ends =
            |id: &str, waiting: &Waiting| id == result.command_id && waiting.to == connection;
        self.end(hub, user, ends, |_| result.clone());
    }

    /// Fails at once every command waiting on `user`'s connection `connection`, now closed.
    pub(in crate::api::player) fn fail_commands_to(&self, hub: &Hub, user: i64, connection: u64) {
        let failed = |id| CommandResult::new(id, Status::Failed, Some("The device went away."));
        self.end(hub, user, |_, waiting| waiting.to == connection, failed);
    }

    /// Fails at once every command of `user`'s that went to whatever connection plays, unless
    /// that is `connection`, which has just started playback: carried out late, it could leave
    /// two devices playing (`requirements/realtime.md` §2), even from a connection that has
    /// stopped since. A `setVolume` or a transfer to another device is left to finish on the
    /// connection it went to, even if another connection of that device takes over meanwhile:
    /// it went to the device it names, which may yet carry it out.
    pub(in crate::api::player) fn fail_commands_taken_over_by(
        &self,
        hub: &Hub,
        user: i64,
        connection: u64,
    ) {
        let failed = |id| CommandResult::new(id, Status::Failed, Some("Playback was taken over."));
        let ends = |_: &str, waiting: &Waiting| waiting.for_playback && waiting.to != connection;
        self.end(hub, user, ends, failed);
    }

    /// Tells the sender of `user`'s command `id`, if it still waits, that it failed for
    /// `reason`.
    fn fail(&self, hub: &Hub, user: i64, id: &str, reason: &str) {
        let failed = |id| CommandResult::new(id, Status::Failed, Some(reason));
        self.end(hub, user, |other, _| other == id, failed);
    }

    /// Ends each of `user`'s waiting commands that `ends` picks with the result `result` gives
    /// for its ID, kept for a client that sends it again, and tells its sender.
    fn end(
        &self,
        hub: &Hub,
        user: i64,
        mut ends: impl FnMut(&str, &Waiting) -> bool,
        result: impl Fn(String) -> CommandResult,
    ) {
        let mut all = self.commands.lock().recover();
        let Some(commands) = all.get_mut(&user) else { return };
        for (id, waiting) in commands.waiting.extract_if(|id, waiting| ends(id, waiting)) {
            waiting.timer.abort();
            let result = result(id);
            if commands.finished.len() == REMEMBERED {
                commands.finished.pop_front();
            }
            commands.finished.push_back(result.clone());
            deliver(hub, user, waiting.reply, result);
        }
    }
}

/// Refuses a command there is no device to carry out.
fn unavailable(reason: &str) -> Problem {
    Problem::new(Code::DeviceUnavailable).detail(reason)
}

/// Sends `result` to `user`'s `reply`, if it is still there to hear it.
fn deliver(hub: &Hub, user: i64, reply: Reply, result: CommandResult) {
    match reply {
        Reply::Connection(id) => hub.send(user, id, &ServerMessage::CommandResult { result }),
        Reply::Request(request) => {
            if request.send(result).is_err() {
                tracing::debug!("command result dropped: its HTTP request is gone");
            }
        }
    }
}
