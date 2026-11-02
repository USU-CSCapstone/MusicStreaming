//! Starting a queue from track IDs: `startQueue`.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::player::Players;
use crate::api::realtime::{Hub, ServerMessage};
use crate::api::scope::Caller;
use crate::api::{Id, Problem, Uuid, extract, plays};
use crate::db::{Database, reaches};
use crate::queue::{KeepError, NewQueue, Queues, list_hash};

/// The spec's `StartQueue`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Start {
    queue_id: Uuid,
    context: Value,
    source: NewSource,
    /// The index of the track that plays first.
    start: u64,
}

/// The tracks of a new source, as the device resolved them.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewSource {
    key: Uuid,
    library_id: String,
    tracks: Vec<String>,
}

/// The spec's `StartedQueue`: the list the server holds for the start, which the device attaches
/// its own tracks to under the hash, so it never downloads them back.
#[derive(Serialize)]
pub struct Started {
    list: String,
    length: usize,
}

/// Starts a queue that plays the request's tracks from `start`, parking the one it replaces
/// (kept if it was adjusted, `requirements/queue.md` §6), and tells the account's connections.
/// Refused, `422`, unless every track is in the library and the caller reaches the library,
/// whether or not what is refused exists, so no one learns what a library they cannot reach
/// holds (`requirements/users.md` §10). A request for a queue the account already has, active or
/// kept, by `queueId`, changes nothing and is answered as the first was, so a device may send it
/// again.
pub async fn start(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    State(players): State<Arc<Players>>,
    State(hub): State<Arc<Hub>>,
    caller: Caller,
    extract::Json(request): extract::Json<Start>,
) -> Result<Json<Started>, Problem> {
    // The context is checked as a play's is, and kept as sent.
    plays::json_text(&request.context, "A queue's context", true)?;
    let NewSource { key, library_id, tracks } = request.source;
    let library = Id::canonical(&library_id).ok_or_else(refused)?;
    let ids = check_tracks(&db, &caller, library, &tracks).await?;
    let length = ids.len();
    let tracks: Arc<[u64]> = ids.into();

    let user = caller.user;
    let queue_id = request.queue_id;
    let new = NewQueue {
        queue_id: queue_id.clone(),
        context: request.context,
        key: key.as_str().to_owned(),
        library_id: library.0,
        tracks: tracks.clone(),
        recipe: None,
        at: request.start,
    };
    // Under the queues' lock, so lock order is queues before player state.
    let place = move || players.position(user);
    let list = match queues.start(&db, user, new, place).await {
        Ok(list) => {
            hub.broadcast(user, &ServerMessage::QueueReplaced { queue_id });
            list
        }
        // A retry: the answer is the one the first start gave, which the same tracks hash to.
        Err(KeepError::Exists) => list_hash(&tracks),
        Err(KeepError::NotInList(error)) => return Err(Problem::invalid(error.to_string())),
        Err(KeepError::Db(error)) => return Err(error.into()),
        Err(KeepError::NotFound | KeepError::Active) => unreachable!("only restores and deletes"),
    };
    Ok(Json(Started { list, length }))
}

/// The refusal for a track out of the library or a library out of reach, the same whether or
/// not it exists.
fn refused() -> Problem {
    Problem::invalid("Every track must be in the library, and the library one you reach.")
}

/// `tracks` as IDs, once every one is checked to be in `library`, which the caller must reach.
/// Refused, `422`, as a start is.
async fn check_tracks(
    db: &Database,
    caller: &Caller,
    library: Id,
    tracks: &[String],
) -> Result<Vec<u64>, Problem> {
    let ids = tracks
        .iter()
        .map(|track| Id::canonical(track).map(|Id(id)| id.cast_unsigned()))
        .collect::<Option<Vec<u64>>>()
        .ok_or_else(refused)?;
    let mut distinct = ids.clone();
    distinct.sort_unstable();
    distinct.dedup();

    // Counted by the database from the IDs as one JSON array, which takes one statement
    // however many there are.
    let json = serde_json::to_string(&distinct).map_err(|_| refused())?;
    let (admin, user) = (caller.admin, caller.user);
    let found: usize = db
        .read(move |conn| {
            conn.prepare_cached(concat!(
                "SELECT count(*) FROM tracks t \
                 WHERE t.library_id = ?1 AND t.id IN (SELECT value FROM json_each(?2)) AND ",
                reaches!(admin: "?3", user: "?4", library: "?1")
            ))?
            .query_row(params![library.0, json, admin, user], |row| row.get(0))
        })
        .await?;
    if found == distinct.len() { Ok(ids) } else { Err(refused()) }
}
