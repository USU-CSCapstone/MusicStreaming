//! The account's queue: `getQueue`, `startQueue` and `editQueue` (`requirements/queue.md`, `design/queue.md`).
//!
//! The server keeps each account's queue in memory and saves it in the background, as it does
//! playback state. A device that starts a queue chooses its `queueId` and its source's key, so
//! it can apply the start at once; the server checks that the caller reaches every track the
//! source names, then replaces the account's queue (`requirements/queue.md` §6) and carries the
//! Manual Queue over. The queue itself, and every rule about it, is the shared core's.

mod edits;
mod saved;
mod start;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jewelcase_core::{Queue, Recover};
use serde::Serialize;
use serde_json::Value;

use super::scope::Caller;
use super::{Code, Problem, Uuid};
pub use edits::edit;
pub use start::start;

/// Each account's queue, for those that have started one.
pub struct Queues {
    accounts: Mutex<HashMap<i64, Account>>,
    /// Drawn as the queues are made, and part of every `ETag`. A crash can lose edits a device
    /// already heard, and the restored queue can then reach the same version with other
    /// contents; a new value makes every tag from before the restart a mismatch.
    boot: u64,
}

impl Default for Queues {
    /// No queues, and a new value for `boot`.
    fn default() -> Self {
        Queues::holding(HashMap::new())
    }
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
    /// The latest edits applied, oldest first, by `editId` and the version each made, so one
    /// sent again is applied once. Not saved: a device sends an edit again within moments.
    done: VecDeque<(Uuid, u32)>,
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
    /// Queues holding `accounts`, with a new value for `boot`; if the system's random source
    /// cannot be read, the clock stands in.
    fn holding(accounts: HashMap<i64, Account>) -> Self {
        let boot = getrandom::u64().unwrap_or_else(|_| crate::db::now_ms().unsigned_abs());
        Queues { accounts: Mutex::new(accounts), boot }
    }

    /// `user`'s snapshot, or `None` if they have no queue: its `ETag`, and its JSON unless
    /// `known` (an `If-None-Match` value) already names this queue and version, which is checked
    /// before anything is serialized.
    fn snapshot(&self, user: i64, known: Option<&str>) -> Option<(String, Option<Vec<u8>>)> {
        // Only copies are made under the lock (the sources are shared, not copied), so
        // serializing a large queue does not hold up every other account.
        let (etag, queue_id, version, context, queue) = {
            let accounts = self.lock();
            let account = accounts.get(&user)?;
            let etag =
                format!("\"{}.{}.{}\"", self.boot, account.queue_id.as_str(), account.version);
            if known.is_some_and(|known| known.split(',').any(|tag| tag.trim() == etag)) {
                return Some((etag, None));
            }
            (
                etag,
                account.queue_id.clone(),
                account.version,
                account.context.clone(),
                account.queue.clone(),
            )
        };
        let snapshot = Snapshot { queue_id: &queue_id, version, context: &context, state: &queue };
        // A queue takes only what its fields hold to serialize, so this cannot fail.
        let body = serde_json::to_vec(&snapshot).ok()?;
        Some((etag, Some(body)))
    }

    /// Each account's queue, locked.
    fn lock(&self) -> MutexGuard<'_, HashMap<i64, Account>> {
        self.accounts.lock().recover()
    }
}

/// `getQueue`: the caller's own queue, or `404` before they have started one. It carries an
/// `ETag` naming the queue and its version, and `304` with no body answers an `If-None-Match`
/// that names them, so a device that reconnects downloads nothing it has. Serialized off the
/// async threads, since a queue of hundreds of thousands of tracks takes a while.
pub async fn get(
    State(queues): State<Arc<Queues>>,
    caller: Caller,
    headers: HeaderMap,
) -> Result<Response, Problem> {
    let known = headers.get(header::IF_NONE_MATCH).and_then(|value| value.to_str().ok());
    let known = known.map(str::to_owned);
    let found = tokio::task::spawn_blocking(move || queues.snapshot(caller.user, known.as_deref()))
        .await
        .map_err(|_| Problem::new(Code::Internal))?;
    let (etag, body) = found.ok_or_else(Problem::not_found)?;
    Ok(match body {
        Some(body) => ([(header::CONTENT_TYPE, "application/json"), (header::ETAG, &etag)], body)
            .into_response(),
        None => (StatusCode::NOT_MODIFIED, [(header::ETAG, &etag)]).into_response(),
    })
}

#[cfg(test)]
mod tests;
