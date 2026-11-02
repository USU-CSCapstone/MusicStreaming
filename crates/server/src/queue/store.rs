//! The in-memory store: per account, the active queue and the lists not yet saved.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use jewelcase_core::{QueueState, Recover};
use rusqlite::Connection;
use serde_json::Value;

use super::hash::list_hash;
use crate::api::Uuid;

/// Each account's active queue, for those that have one.
pub struct Queues {
    pub(super) accounts: Mutex<HashMap<i64, Account>>,
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
    /// Whether anything changed since the state was last taken to be saved.
    pub(super) changed: bool,
}

/// A list waiting to be saved.
#[derive(Clone)]
pub(super) struct Held {
    pub(super) library_id: i64,
    pub(super) tracks: Arc<[u64]>,
}

impl Account {
    /// An account whose active queue is `state`, changed, so the next save writes it.
    #[cfg_attr(not(test), expect(dead_code, reason = "starting a queue is a later step"))]
    pub(super) fn new(queue_id: Uuid, context: Value, state: QueueState) -> Account {
        Account { queue_id, version: 0, context, state, lists: HashMap::new(), changed: true }
    }

    /// Holds `tracks`, a list of `library_id`, until it is saved, and returns its hash. The caller
    /// marks the account changed with the edit that uses it.
    #[cfg_attr(not(test), expect(dead_code, reason = "starting a queue is a later step"))]
    pub(super) fn hold(&mut self, library_id: i64, tracks: Arc<[u64]>) -> String {
        let hash = list_hash(&tracks);
        self.lists.insert(hash.clone(), Held { library_id, tracks });
        hash
    }
}

impl Queues {
    /// Queues holding `accounts`.
    pub(super) fn holding(accounts: HashMap<i64, Account>) -> Queues {
        Queues { accounts: Mutex::new(accounts) }
    }

    /// Every account's active queue as saved, as the server starts. Lists are not loaded: they
    /// stay in the database until something reads them. A queue that cannot be read back is
    /// left out and logged, so one damaged row does not keep the server from starting.
    pub async fn load(db: &crate::db::Database) -> Result<Queues, crate::db::DbError> {
        Ok(Queues::holding(db.read(load).await?))
    }

    /// Each account's queues, locked.
    pub(super) fn lock(&self) -> MutexGuard<'_, HashMap<i64, Account>> {
        self.accounts.lock().recover()
    }
}

/// Each account's active queue, by user ID.
fn load(conn: &Connection) -> rusqlite::Result<HashMap<i64, Account>> {
    let mut select = conn.prepare(
        "SELECT user_id, queue_id, version, context, state FROM queues WHERE active = 1",
    )?;
    let rows = select.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, u32>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut accounts = HashMap::new();
    for row in rows {
        let (user, queue_id, version, context, state) = row?;
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
        accounts.insert(user, Account { queue_id, version, context, state, lists, changed: false });
    }
    Ok(accounts)
}
