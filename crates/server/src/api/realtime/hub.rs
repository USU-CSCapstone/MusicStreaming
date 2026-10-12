//! Which of each account's devices hold a realtime connection (`requirements/realtime.md` §1),
//! and the way to reach them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::{Revoked, ServerMessage};

#[derive(Default)]
pub struct Hub {
    /// Each account's open connections. A device may hold several, such as two browser tabs
    /// sharing one sign-in.
    accounts: Mutex<HashMap<i64, Vec<Connection>>>,
}

struct Connection {
    device: i64,
    /// Messages for the connection to send its client.
    outbox: UnboundedSender<ServerMessage>,
}

impl Hub {
    /// Counts `device` of `user` as connected until the returned [`Presence`] is dropped.
    pub fn connect(self: &Arc<Self>, user: i64, device: i64) -> Presence {
        let (outbox, messages) = unbounded_channel();
        self.lock().entry(user).or_default().push(Connection { device, outbox: outbox.clone() });
        Presence { hub: self.clone(), user, outbox, messages }
    }

    /// The devices of `user` that hold a connection, once for each they hold.
    pub fn connected(&self, user: i64) -> Vec<i64> {
        let accounts = self.lock();
        accounts.get(&user).into_iter().flatten().map(|connection| connection.device).collect()
    }

    /// Tells every connection `device` of `user` holds that it was signed out, which closes it.
    /// Call it once the sign-out is committed: a connection that joins after this checks then.
    pub fn revoke(&self, user: i64, device: i64, reason: Revoked) {
        let accounts = self.lock();
        for connection in accounts.get(&user).into_iter().flatten() {
            if connection.device == device {
                // Fails only for a connection already closing, which needs no telling.
                let _ = connection.outbox.send(ServerMessage::SessionRevoked { reason });
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<i64, Vec<Connection>>> {
        self.accounts.lock().expect("realtime hub lock")
    }
}

/// One open connection, counted until it is dropped.
pub struct Presence {
    hub: Arc<Hub>,
    user: i64,
    /// Tells this connection's entry in the hub apart from its device's others.
    outbox: UnboundedSender<ServerMessage>,
    messages: UnboundedReceiver<ServerMessage>,
}

impl Presence {
    /// The next message the hub has for this connection's client.
    pub(super) async fn next(&mut self) -> Option<ServerMessage> {
        self.messages.recv().await
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        let mut accounts = self.hub.lock();
        let Some(connections) = accounts.get_mut(&self.user) else { return };
        connections.retain(|connection| !connection.outbox.same_channel(&self.outbox));
        if connections.is_empty() {
            accounts.remove(&self.user);
        }
    }
}
