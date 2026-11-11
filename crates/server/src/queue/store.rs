//! The in-memory store: per account, the active queue and the lists not yet saved.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use jewelcase_core::{ItemId, QueueState, Recover};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::hash::list_hash;
use crate::api::Uuid;

/// Each account's active queue, for those that have one.
pub struct Queues {
    pub(super) accounts: Mutex<HashMap<i64, Account>>,
    /// Drawn as the server starts, and part of every snapshot's `ETag`. A crash can lose edits
    /// a device already heard, and the restored queue can then reach the same version with other
    /// contents; a new value makes every tag from before the restart a mismatch.
    pub(super) boot: u64,
}

impl Default for Queues {
    /// No queues, and a new value for `boot`.
    fn default() -> Self {
        Queues::holding(HashMap::new())
    }
}

/// One account's active queue and what the server needs to know about it.
pub(super) struct Account {
    /// Chosen by the device that started the queue.
    pub(super) queue_id: Uuid,
    /// Zero as the queue starts.
    pub(super) version: u32,
    /// What the queue started from, as the device sent it.
    pub(super) context: Value,
    /// Run with no list attached: its checks need only lengths and positions.
    pub(super) state: QueueState,
    /// Lists the database may not hold yet, by hash. They belong to the account, not a queue.
    pub(super) lists: HashMap<String, Held>,
    /// Where playback was when the queue was last parked, kept while it is active so that
    /// parking it again, with the player somewhere else, does not lose it.
    pub(super) position: Option<Position>,
    /// Queues replaced by a start or restore and not yet saved: each is kept or deleted by the
    /// next save, whichever its `adjusted` says.
    pub(super) parked: Vec<Queue>,
    /// Whether anything changed since the state was last taken to be saved.
    pub(super) changed: bool,
}

/// The item and point in its track that playback was at when a queue was parked, so a restore
/// resumes there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Position {
    /// The item, as the player state names it: `<key>.<position>`.
    pub item: String,
    /// How far into the item's track.
    pub position_ms: u32,
}

/// A queue that is not the active one, as memory holds it before the save writes it.
#[derive(Clone)]
pub(super) struct Queue {
    pub(super) queue_id: Uuid,
    pub(super) version: u32,
    pub(super) context: Value,
    pub(super) state: QueueState,
    pub(super) position: Option<Position>,
}

/// A list waiting to be saved.
#[derive(Clone)]
pub(super) struct Held {
    pub(super) library_id: i64,
    pub(super) tracks: Arc<[u64]>,
}

impl Account {
    /// An account whose active queue is `state`, changed, so the next save writes it.
    pub(super) fn new(queue_id: Uuid, context: Value, state: QueueState) -> Account {
        Account {
            queue_id,
            version: 0,
            context,
            state,
            lists: HashMap::new(),
            position: None,
            parked: Vec::new(),
            changed: true,
        }
    }

    /// An account whose active queue is `queue`, changed, so the next save writes it.
    pub(super) fn from_queue(queue: Queue) -> Account {
        let mut account = Account::new(queue.queue_id, queue.context, queue.state);
        account.version = queue.version;
        account.position = queue.position;
        account
    }

    /// Makes `next` the active queue and parks the one it replaces, which keeps the position
    /// `place` gives if that names one of its items, and otherwise the one it had. Marks the
    /// account changed.
    pub(super) fn park_for(&mut self, next: Queue, place: Option<Position>) {
        let mut old = self.swap(next);
        let names_item =
            |at: &Position| at.item.parse::<ItemId>().is_ok_and(|item| old.state.holds_item(&item));
        if let Some(at) = place.filter(names_item) {
            old.position = Some(at);
        }
        self.parked.push(old);
        self.changed = true;
    }

    /// Puts `queue` in place of the active queue, and returns the one replaced.
    pub(super) fn swap(&mut self, queue: Queue) -> Queue {
        Queue {
            queue_id: std::mem::replace(&mut self.queue_id, queue.queue_id),
            version: std::mem::replace(&mut self.version, queue.version),
            context: std::mem::replace(&mut self.context, queue.context),
            state: std::mem::replace(&mut self.state, queue.state),
            position: std::mem::replace(&mut self.position, queue.position),
        }
    }

    /// A copy of the active queue.
    pub(super) fn active(&self) -> Queue {
        Queue {
            queue_id: self.queue_id.clone(),
            version: self.version,
            context: self.context.clone(),
            state: self.state.clone(),
            position: self.position.clone(),
        }
    }

    /// Whether `id` names the active queue or one parked and not yet saved.
    pub(super) fn names(&self, id: &Uuid) -> bool {
        self.queue_id == *id || self.parked.iter().any(|queue| queue.queue_id == *id)
    }

    /// Holds `tracks`, a list of `library_id`, until it is saved, and returns its hash. The caller
    /// marks the account changed with the edit that uses it.
    pub(super) fn hold(&mut self, library_id: i64, tracks: Arc<[u64]>) -> String {
        let hash = list_hash(&tracks);
        self.lists.insert(hash.clone(), Held { library_id, tracks });
        hash
    }
}

impl Queues {
    /// Queues holding `accounts`, with a new value for `boot`; if the system's random source
    /// cannot be read, the clock stands in.
    pub(super) fn holding(accounts: HashMap<i64, Account>) -> Queues {
        let boot = getrandom::u64().unwrap_or_else(|_| crate::db::now_ms().unsigned_abs());
        Queues { accounts: Mutex::new(accounts), boot }
    }

    /// Every account's active queue as saved, as the server starts. Lists are not loaded: they
    /// stay in the database until something reads them. A queue that cannot be read back is
    /// left out and logged, so one damaged row does not keep the server from starting.
    pub async fn load(db: &crate::db::Database) -> Result<Queues, crate::db::DbError> {
        Ok(Queues::holding(db.read(load).await?))
    }

    /// Whether `user`'s active queue holds `item`, in any of its sections. An account with no
    /// queue holds none, and nor does an item that is not `<key>.<position>`.
    pub fn holds(&self, user: i64, item: &str) -> bool {
        let Ok(item) = item.parse::<ItemId>() else { return false };
        self.lock().get(&user).is_some_and(|account| account.state.holds_item(&item))
    }

    /// Each account's queues, locked.
    pub(super) fn lock(&self) -> MutexGuard<'_, HashMap<i64, Account>> {
        self.accounts.lock().recover()
    }
}

/// Each account's active queue, by user ID.
fn load(conn: &Connection) -> rusqlite::Result<HashMap<i64, Account>> {
    let mut select = conn.prepare(
        "SELECT user_id, queue_id, version, context, state, position FROM queues WHERE active = 1",
    )?;
    let rows = select.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, u32>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut accounts = HashMap::new();
    for row in rows {
        let (user, queue_id, version, context, state, position) = row?;
        let read = (
            Uuid::try_from(queue_id),
            serde_json::from_str::<Value>(&context),
            serde_json::from_str::<QueueState>(&state),
        );
        let (Ok(queue_id), Ok(context), Ok(state)) = read else {
            tracing::warn!(user, "cannot read an account's saved queue");
            continue;
        };
        let lists = HashMap::new();
        let position = position.and_then(|text| serde_json::from_str(&text).ok());
        let parked = Vec::new();
        accounts.insert(
            user,
            Account { queue_id, version, context, state, lists, position, parked, changed: false },
        );
    }
    Ok(accounts)
}
