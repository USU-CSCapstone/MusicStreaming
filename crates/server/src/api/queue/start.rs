//! Starting a queue from track IDs: `startQueue`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use jewelcase_core::{Queue, Source};
use rusqlite::params;
use serde::Deserialize;
use serde_json::Value;

use super::{Account, Queues};
use crate::api::scope::Caller;
use crate::api::{Id, Problem, Uuid, extract, plays};
use crate::db::{Database, reaches};

/// The spec's `StartQueue`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Start {
    queue_id: Uuid,
    context: Value,
    source: NewSource,
    start: usize,
    shuffle: Option<Shuffle>,
}

/// The source a queue starts from, as the device resolved it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewSource {
    key: Uuid,
    library_id: String,
    tracks: Vec<String>,
}

/// Start with the context shuffled.
#[derive(Deserialize)]
struct Shuffle {
    seed: u32,
}

/// Replaces the caller's queue with one that plays the request's source from `start`, carrying
/// the Manual Queue and the repeat mode over. Refuses the request, `422`, unless every track is in the library and
/// the caller reaches the library, whether or not what is refused exists, so no one learns what
/// a library they cannot reach holds (`requirements/users.md` §10). A request for the queue the
/// caller already has, by `queueId`, changes nothing, so a device may send it again.
pub async fn start(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    caller: Caller,
    extract::Json(request): extract::Json<Start>,
) -> Result<StatusCode, Problem> {
    // The context is checked as a play's is, and kept as sent.
    plays::json_text(&request.context, "A queue's context", true)?;
    let refused =
        || Problem::invalid("Every track must be in the library, and the library one you reach.");
    let library = Id::canonical(&request.source.library_id).ok_or_else(refused)?;
    let mut distinct = request
        .source
        .tracks
        .iter()
        .map(|track| Id::canonical(track).map(|Id(id)| id))
        .collect::<Option<Vec<i64>>>()
        .ok_or_else(refused)?;
    distinct.sort_unstable();
    distinct.dedup();

    // Counted by the database from the IDs as one JSON array, which takes one statement
    // however many there are.
    let ids = serde_json::to_string(&distinct).map_err(|_| refused())?;
    let found: usize = db
        .read(move |conn| {
            conn.prepare_cached(concat!(
                "SELECT count(*) FROM tracks t \
                 WHERE t.library_id = ?1 AND t.id IN (SELECT value FROM json_each(?2)) AND ",
                reaches!(admin: "?3", user: "?4", library: "?1")
            ))?
            .query_row(params![library.0, ids, caller.admin, caller.user], |row| row.get(0))
        })
        .await?;
    if found != distinct.len() {
        return Err(refused());
    }

    let NewSource { key, library_id, tracks } = request.source;
    let source = Source { key: key.as_str().to_owned(), library_id, tracks };
    let seed = request.shuffle.map(|shuffle| shuffle.seed);
    queues.replace(caller.user, request.queue_id, request.context, source, request.start, seed)?;
    Ok(StatusCode::NO_CONTENT)
}

impl Queues {
    /// Makes `user`'s queue one that plays `source` from `start`, shuffled with `seed` if given,
    /// and keeps the Manual Queue and repeat mode they had. Does nothing if their queue is `queue_id` already.
    pub(super) fn replace(
        &self,
        user: i64,
        queue_id: Uuid,
        context: Value,
        source: Source,
        start: usize,
        seed: Option<u32>,
    ) -> Result<(), Problem> {
        let invalid = |error: jewelcase_core::QueueError| Problem::invalid(error.to_string());
        let mut accounts = self.lock();
        let before = accounts.get(&user);
        if before.is_some_and(|account| account.queue_id == queue_id) {
            return Ok(());
        }
        let queue = Queue::launch(source, start, seed, before.map(|account| &account.queue))
            .map_err(invalid)?;
        // None of this queue is in the database, whatever an earlier queue left there, so the
        // next save replaces it.
        let saved = None;
        accounts
            .insert(user, Account { queue_id, version: 0, context, queue, unsaved: true, saved });
        Ok(())
    }
}
