//! Which of each account's devices hold a realtime connection (`requirements/realtime.md` §1).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Default)]
pub struct Hub {
    /// Each account's open connections, as the device each is from. A device may hold several,
    /// such as two browser tabs sharing one sign-in.
    accounts: Mutex<HashMap<i64, Vec<i64>>>,
}

impl Hub {
    /// Counts `device` of `user` as connected until the returned [`Presence`] is dropped.
    pub fn connect(self: &Arc<Self>, user: i64, device: i64) -> Presence {
        self.lock().entry(user).or_default().push(device);
        Presence { hub: self.clone(), user, device }
    }

    /// The devices of `user` that hold a connection, once for each they hold.
    pub fn connected(&self, user: i64) -> Vec<i64> {
        self.lock().get(&user).cloned().unwrap_or_default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<i64, Vec<i64>>> {
        self.accounts.lock().expect("realtime hub lock")
    }
}

/// One open connection, counted until it is dropped.
pub struct Presence {
    hub: Arc<Hub>,
    user: i64,
    device: i64,
}

impl Drop for Presence {
    fn drop(&mut self) {
        let mut accounts = self.hub.lock();
        let Some(devices) = accounts.get_mut(&self.user) else { return };
        if let Some(i) = devices.iter().position(|&device| device == self.device) {
            devices.swap_remove(i);
        }
        if devices.is_empty() {
            accounts.remove(&self.user);
        }
    }
}
