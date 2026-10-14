//! Passing commands on to the connection that carries them out, and their results back to
//! their senders.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::task::AbortHandle;

use super::{Action, CommandResult, PlayerCommand, Refused, Status};
use crate::api::Id;
use crate::api::authenticate::Caller;
use crate::api::player::{Players, Status as PlayStatus};
use crate::api::plays::is_uuid;
use crate::api::realtime::{Hub, ServerMessage};

/// How long a connection has to answer a command before it counts as failed.
const ANSWER_WITHIN: Duration =
    if cfg!(test) { Duration::from_secs(2) } else { Duration::from_secs(5) };

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

impl Commands {
    /// Ends `waiting` with `result`, kept for a client that sends it again, and answers where
    /// to send that.
    fn finish(&mut self, waiting: Waiting, result: &CommandResult) -> Reply {
        waiting.timer.abort();
        if self.finished.len() == REMEMBERED {
            self.finished.pop_front();
        }
        self.finished.push_back(result.clone());
        waiting.reply
    }
}

impl Players {
    /// Passes `command` from `caller` on to the connection that carries it out, and answers what
    /// to tell the sender now: `pending`, or the result of a command sent again that has
    /// finished.
    pub(super) fn command(
        self: &Arc<Self>,
        hub: &Arc<Hub>,
        caller: Caller,
        reply: Reply,
        mut command: PlayerCommand,
    ) -> Result<CommandResult, Refused> {
        if !is_uuid(&command.command_id) {
            return Err(Refused::Invalid("commandId must be a UUID."));
        }
        if let Action::SetVolume { volume } = command.action {
            if command.device_id.is_none() {
                let reason = "setVolume must name its deviceId: volume belongs to a device.";
                return Err(Refused::Invalid(reason));
            }
            if !(0.0..=1.0).contains(&volume) {
                return Err(Refused::Invalid("volume must be from 0 to 1."));
            }
        }
        let id = command.command_id.clone();
        let pending = CommandResult::new(id.clone(), Status::Pending, None);
        let accounts = self.lock();
        let mut all = self.commands.lock().expect("player commands lock");
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
        let to = if let Action::Transfer { .. } = command.action {
            let Some(device) = command.device_id.as_deref() else {
                return Err(Refused::Invalid("transfer must name its deviceId."));
            };
            if state.and_then(|state| state.track_id).is_none() {
                return Err(Refused::Unavailable("Nothing has played to move."));
            }
            let device = Id::canonical(device);
            let active = state.filter(|state| device.is_some() && state.active_device_id == device);
            if let Some(active) = active {
                // Moved where it already is: done, unless it is to play there and is paused,
                // which is as good as asking that connection to play.
                let play = matches!(command.action, Action::Transfer { play: true });
                match active.active_connection {
                    Some(to) if play && active.status != PlayStatus::Playing => {
                        command.action = Action::Play;
                        to
                    }
                    _ => return Ok(CommandResult::new(id, Status::Applied, None)),
                }
            } else {
                // A device of another account is as not connected as one can be.
                let newest = device.and_then(|device| hub.newest(caller.user, device.0));
                let Some(to) = newest else {
                    return Err(Refused::Unavailable("That device is not connected."));
                };
                to
            }
        } else {
            let playing =
                state.and_then(|state| Some((state.active_connection?, state.active_device_id?)));
            let Some((to, device)) = playing else {
                return Err(Refused::Unavailable("Nothing is playing."));
            };
            if command.device_id.as_deref().is_some_and(|id| Id::canonical(id) != Some(device)) {
                return Err(Refused::Unavailable("That device is not the one playing."));
            }
            to
        };
        if commands.waiting.len() >= MAX_WAITING {
            return Err(Refused::Busy);
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
        commands.waiting.insert(id, Waiting { reply, to, timer: timer.abort_handle() });
        Ok(pending)
    }

    /// Passes `result`, from `user`'s connection `connection`, back to the sender of the
    /// command it answers, if it is the connection that command went to.
    pub fn answered(&self, hub: &Hub, user: i64, connection: u64, mut result: CommandResult) {
        if result.status == Status::Pending {
            return;
        }
        result.reason = result.reason.map(|reason| reason.chars().take(MAX_REASON).collect());
        let mut all = self.commands.lock().expect("player commands lock");
        let Some(commands) = all.get_mut(&user) else { return };
        let Entry::Occupied(entry) = commands.waiting.entry(result.command_id.clone()) else {
            return;
        };
        if entry.get().to != connection {
            return;
        }
        let waiting = entry.remove();
        let reply = commands.finish(waiting, &result);
        drop(all);
        deliver(hub, user, reply, result);
    }

    /// Fails at once every command waiting on `user`'s connection `connection`, now closed.
    pub(in crate::api::player) fn fail_commands_to(&self, hub: &Hub, user: i64, connection: u64) {
        let replies: Vec<_> = {
            let mut all = self.commands.lock().expect("player commands lock");
            let Some(commands) = all.get_mut(&user) else { return };
            let gone: Vec<_> =
                commands.waiting.extract_if(|_, waiting| waiting.to == connection).collect();
            let reason = Some("The device went away.");
            gone.into_iter()
                .map(|(id, waiting)| {
                    let result = CommandResult::new(id, Status::Failed, reason);
                    (commands.finish(waiting, &result), result)
                })
                .collect()
        };
        for (reply, result) in replies {
            deliver(hub, user, reply, result);
        }
    }

    /// Tells the sender of `user`'s command `id`, if it still waits, that it failed for
    /// `reason`.
    fn fail(&self, hub: &Hub, user: i64, id: &str, reason: &str) {
        let result = CommandResult::new(id.to_owned(), Status::Failed, Some(reason));
        let reply = {
            let mut all = self.commands.lock().expect("player commands lock");
            let Some(commands) = all.get_mut(&user) else { return };
            let Some(waiting) = commands.waiting.remove(id) else { return };
            commands.finish(waiting, &result)
        };
        deliver(hub, user, reply, result);
    }
}

/// Sends `result` to `user`'s `reply`, if it is still there to hear it.
fn deliver(hub: &Hub, user: i64, reply: Reply, result: CommandResult) {
    match reply {
        Reply::Connection(id) => hub.send(user, id, &ServerMessage::CommandResult { result }),
        Reply::Request(request) => {
            let _ = request.send(result);
        }
    }
}
