//! One run at a time for each plugin in each library (`design/hooks.md` §1): its hooks, Run now,
//! and settings checks there take turns, since each may read and write the plugin's state there.
//! A user's personal settings check has no state, and runs in a turn of that user's instead,
//! whose queue it joins in the plugin's turn at the server level, so an uninstall holding that
//! turn can wait for every user's.
//! Runs in different libraries, and of different plugins, still go side by side.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::OwnedMutexGuard;

use super::settings::SERVER;
use jewelcase_core::Recover;

/// A plugin's turn to run, held until it is dropped.
pub(super) type Turn = OwnedMutexGuard<()>;

/// The lock that hands out one plugin's turns in one [`Queue`], first come first served.
type Pair = Arc<tokio::sync::Mutex<()>>;

/// Where a plugin's runs take turns.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Queue {
    /// A library, or the server level as `settings::SERVER`.
    Level(i64),
    /// One user's personal settings checks.
    Personal(i64),
}

/// Each plugin and queue that has taken a turn, with its [`Pair`].
#[derive(Default)]
pub(super) struct Turns(Mutex<HashMap<(String, Queue), Pair>>);

impl Turns {
    /// Waits for `plugin`'s turn in `library`.
    pub(super) async fn take(&self, plugin: &str, library: i64) -> Turn {
        self.pair(plugin, Queue::Level(library)).lock_owned().await
    }

    /// Waits for `plugin`'s turn to check `user`'s personal settings. It joins that queue in the
    /// plugin's turn at the server level, and lets that go before it waits.
    pub(super) async fn take_personal(&self, plugin: &str, user: i64) -> Turn {
        let pair = {
            let _server = self.take(plugin, SERVER).await;
            self.pair(plugin, Queue::Personal(user))
        };
        pair.lock_owned().await
    }

    /// Waits for every turn of `plugin`'s personal settings checks, one after another. Called
    /// in the plugin's turn at the server level, which [`Turns::take_personal`] needs to join a
    /// queue, so no check is under way once it returns, and none starts while that turn is held.
    pub(super) async fn take_every_personal(&self, plugin: &str) -> Vec<Turn> {
        let pairs: Vec<Pair> = {
            let pairs = self.0.lock().recover();
            let personal =
                |(id, queue): &(String, Queue)| id == plugin && matches!(queue, Queue::Personal(_));
            pairs.iter().filter(|(key, _)| personal(key)).map(|(_, pair)| pair.clone()).collect()
        };
        let mut turns = Vec::with_capacity(pairs.len());
        for pair in pairs {
            turns.push(pair.lock_owned().await);
        }
        turns
    }

    /// The lock for `plugin`'s turns in `queue`, made on first use.
    fn pair(&self, plugin: &str, queue: Queue) -> Pair {
        let mut pairs = self.0.lock().recover();
        pairs.entry((plugin.to_owned(), queue)).or_default().clone()
    }

    /// How many runs hold or wait for `plugin`'s turn in `library`.
    #[cfg(test)]
    pub(super) fn waiting(&self, plugin: &str, library: i64) -> usize {
        self.count(plugin, Queue::Level(library))
    }

    /// How many checks, or uninstalls, hold or wait for `plugin`'s turn for `user`.
    #[cfg(test)]
    pub(super) fn waiting_personal(&self, plugin: &str, user: i64) -> usize {
        self.count(plugin, Queue::Personal(user))
    }

    /// How many hold or wait for `plugin`'s turn in `queue`.
    #[cfg(test)]
    fn count(&self, plugin: &str, queue: Queue) -> usize {
        let pairs = self.0.lock().recover();
        // Each holding or waiting for the turn keeps the pair, beside the map's own.
        pairs.get(&(plugin.to_owned(), queue)).map_or(0, |pair| Arc::strong_count(pair) - 1)
    }
}
