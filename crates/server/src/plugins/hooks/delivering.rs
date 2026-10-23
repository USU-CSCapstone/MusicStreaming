//! One delivery at a time for each plugin, library, hook, and user.

use std::sync::Arc;

use jewelcase_plugins::Permission;

use super::Due;
use crate::plugins::Plugins;

/// The plugin, library, hook, and user of a delivery.
type Key = (String, i64, Permission, i64);

/// A delivery under way, holding its key in `Plugins::delivering` until it is dropped, even by a
/// panic.
pub(super) struct Delivering {
    plugins: Arc<Plugins>,
    key: Key,
}

impl Delivering {
    /// Claims `key` for one delivery, or answers `None` if one is already under way.
    pub(super) fn claim(plugins: &Arc<Plugins>, key: Key) -> Option<Delivering> {
        let claimed = plugins.delivering.lock().expect("delivery lock").insert(key.clone());
        claimed.then(|| Delivering { plugins: plugins.clone(), key })
    }

    /// Delivers `due`, then lets the key go.
    pub(super) async fn deliver(self, due: Due) {
        self.plugins.deliver(due).await;
    }
}

impl Drop for Delivering {
    /// Lets the key go, so the next delivery can start.
    fn drop(&mut self) {
        self.plugins.delivering.lock().expect("delivery lock").remove(&self.key);
    }
}
