//! One run at a time for each plugin in each library (`design/hooks.md` §1): its hooks, Run now,
//! and settings checks there take turns, since each may read and write the plugin's state there.
//! A user's personal settings check has no state, and takes no turn.
//! Runs in different libraries, and of different plugins, still go side by side.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::OwnedMutexGuard;

use jewelcase_core::Recover;

/// A plugin's turn to run, held until it is dropped.
pub(super) type Turn = OwnedMutexGuard<()>;

/// The lock that hands out one plugin's turns in one library, first come first served.
type Pair = Arc<tokio::sync::Mutex<()>>;

/// Each plugin and library, or `settings::SERVER`, that has taken a turn, with its [`Pair`].
#[derive(Default)]
pub(super) struct Turns(Mutex<HashMap<(String, i64), Pair>>);

impl Turns {
    /// Waits for `plugin`'s turn in `library`, or at the server level as `settings::SERVER`.
    pub(super) async fn take(&self, plugin: &str, library: i64) -> Turn {
        let pair = {
            let mut pairs = self.0.lock().recover();
            pairs.entry((plugin.to_owned(), library)).or_default().clone()
        };
        pair.lock_owned().await
    }

    /// How many runs hold or wait for `plugin`'s turn in `library`.
    #[cfg(test)]
    pub(super) fn waiting(&self, plugin: &str, library: i64) -> usize {
        let pairs = self.0.lock().recover();
        // Each holding or waiting for the turn keeps the pair, beside the map's own.
        pairs.get(&(plugin.to_owned(), library)).map_or(0, |pair| Arc::strong_count(pair) - 1)
    }
}
