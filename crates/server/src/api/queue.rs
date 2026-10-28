//! The account's queue: `getQueue` and `startQueue` (`requirements/queue.md`, `design/queue.md`).
//!
//! The server keeps each account's queue in memory and saves it in the background, as it does
//! playback state. A device that starts a queue chooses its `queueId` and its source's key, so
//! it can apply the start at once; the server checks that the caller reaches every track the
//! source names, then replaces the account's queue (`requirements/queue.md` §6) and carries the
//! Manual Queue over. The queue itself, and every rule about it, is the shared core's.

mod saved;
mod start;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use jewelcase_core::{Queue, Recover};
use serde::Serialize;
use serde_json::Value;

use super::scope::Caller;
use super::{Code, Problem, Uuid};
pub use start::start;

/// Each account's queue, for those that have started one.
#[derive(Default)]
pub struct Queues {
    accounts: Mutex<HashMap<i64, Account>>,
}

/// One account's queue and what the server needs to know about it.
struct Account {
    /// Chosen by the device that started the queue.
    queue_id: Uuid,
    /// Zero as the queue starts.
    version: u32,
    /// What the queue started from, as the device sent it.
    context: Value,
    queue: Queue,
    /// Whether it changed since it was last saved.
    unsaved: bool,
    /// How many of this queue's sources the database holds, if it holds the queue at all.
    saved: Option<usize>,
}

/// The spec's `QueueSnapshot`: the whole queue, for a device to load into the core.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot<'a> {
    queue_id: &'a Uuid,
    version: u32,
    context: &'a Value,
    state: &'a Queue,
}

impl Queues {
    /// `user`'s snapshot as JSON, or `None` if they have no queue.
    fn snapshot(&self, user: i64) -> Option<Vec<u8>> {
        let accounts = self.lock();
        let account = accounts.get(&user)?;
        let snapshot = Snapshot {
            queue_id: &account.queue_id,
            version: account.version,
            context: &account.context,
            state: &account.queue,
        };
        // A queue takes only what its fields hold to serialize, so this cannot fail.
        serde_json::to_vec(&snapshot).ok()
    }

    /// Each account's queue, locked.
    fn lock(&self) -> MutexGuard<'_, HashMap<i64, Account>> {
        self.accounts.lock().recover()
    }
}

/// `getQueue`: the caller's own queue, or `404` before they have started one. Serialized off
/// the async threads, since a queue of hundreds of thousands of tracks takes a while.
pub async fn get(State(queues): State<Arc<Queues>>, caller: Caller) -> Result<Response, Problem> {
    let body = tokio::task::spawn_blocking(move || queues.snapshot(caller.user))
        .await
        .map_err(|_| Problem::new(Code::Internal))?;
    let body = body.ok_or_else(Problem::not_found)?;
    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}

#[cfg(test)]
mod tests;
