//! Which of each account's devices hold a realtime connection (`requirements/realtime.md` §1),
//! and the way to reach them.
//!
//! The hub closes a connection by letting go of its outbox: the connection sends what is
//! already queued, then closes. A client that falls [`OUTBOX`] messages behind is closed that
//! way rather than queued for without end. On reconnecting it receives current state, which is
//! all it would have caught up to (§6).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::ws::Message;
use tokio::sync::mpsc::{self, Receiver, Sender};

use super::{Revoked, ServerMessage, encode};

/// How many messages a connection may have waiting to be sent.
pub(super) const OUTBOX: usize = 64;

#[derive(Default)]
pub struct Hub {
    /// Each account's open connections. A device may hold several, such as two browser tabs
    /// sharing one sign-in.
    accounts: Mutex<HashMap<i64, Vec<Connection>>>,
    /// Tells connections apart, a device's included.
    next_id: AtomicU64,
    /// Held while a device list is read and sent, so an earlier list never arrives after a
    /// later one.
    device_lists: tokio::sync::Mutex<()>,
}

struct Connection {
    id: u64,
    device: i64,
    /// Messages for the connection to send its client, already encoded.
    outbox: Sender<Message>,
}

impl Hub {
    /// Counts `device` of `user` as connected until the returned [`Presence`] is dropped.
    pub fn connect(self: &Arc<Self>, user: i64, device: i64) -> Presence {
        let (outbox, messages) = mpsc::channel(OUTBOX);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.lock().entry(user).or_default().push(Connection { id, device, outbox });
        Presence { hub: self.clone(), user, id, messages }
    }

    /// The devices of `user` that hold a connection, once for each they hold.
    pub fn connected(&self, user: i64) -> Vec<i64> {
        let accounts = self.lock();
        accounts.get(&user).into_iter().flatten().map(|connection| connection.device).collect()
    }

    /// The connection `device` of `user` opened last, if it holds any.
    pub fn newest(&self, user: i64, device: i64) -> Option<u64> {
        let accounts = self.lock();
        let connections = accounts.get(&user).into_iter().flatten();
        connections.filter(|connection| connection.device == device).map(|c| c.id).max()
    }

    /// Sends `message` to every connection of `user`, closing any whose outbox is full, and
    /// whether it closed any.
    pub fn broadcast(&self, user: i64, message: &ServerMessage) -> bool {
        let message = encode(message);
        self.retain(user, |connection| connection.outbox.try_send(message.clone()).is_ok())
    }

    /// Sends `message` to `user`'s connection `id`, if it is still open, closing it if its
    /// outbox is full. Its `serve` sends its device list once it ends.
    pub fn send(&self, user: i64, id: u64, message: &ServerMessage) {
        let message = encode(message);
        let _ = self.retain(user, |connection| {
            connection.id != id || connection.outbox.try_send(message.clone()).is_ok()
        });
    }

    /// Tells every connection `device` of `user` holds that it was signed out, and closes it.
    /// Call it once the sign-out is committed: a connection that joins after this checks then.
    pub fn revoke(&self, user: i64, device: i64, reason: Revoked) {
        let message = encode(&ServerMessage::SessionRevoked { reason });
        let _ = self.retain(user, |connection| {
            if connection.device != device {
                return true;
            }
            // A full outbox loses the message, but the connection still closes, and the
            // device finds it is signed out when it tries to reconnect.
            let _ = connection.outbox.try_send(message.clone());
            false
        });
    }

    /// Waits for the turn to read and send a device list, which lasts until the guard drops.
    pub async fn device_list_turn(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.device_lists.lock().await
    }

    /// Keeps the connections of `user` that `keep` is true for, closes the rest, and whether
    /// there were any to close.
    fn retain(&self, user: i64, keep: impl FnMut(&Connection) -> bool) -> bool {
        let mut accounts = self.lock();
        let Some(connections) = accounts.get_mut(&user) else { return false };
        let before = connections.len();
        connections.retain(keep);
        let closed = connections.len() < before;
        if connections.is_empty() {
            accounts.remove(&user);
        }
        closed
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<i64, Vec<Connection>>> {
        self.accounts.lock().expect("realtime hub lock")
    }
}

/// One open connection, counted until it is dropped.
pub struct Presence {
    hub: Arc<Hub>,
    user: i64,
    id: u64,
    messages: Receiver<Message>,
}

impl Presence {
    /// The next message the hub has for this connection's client, or `None` once the hub has
    /// closed it.
    pub(super) async fn next(&mut self) -> Option<Message> {
        self.messages.recv().await
    }

    /// Tells this connection apart from every other, its device's included.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether the hub still counts this connection. One it has closed only sends what was
    /// queued, and must not act for its client.
    pub(super) fn is_open(&self) -> bool {
        !self.messages.is_closed()
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        let _ = self.hub.retain(self.user, |connection| connection.id != self.id);
    }
}
