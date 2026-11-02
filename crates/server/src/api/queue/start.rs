//! Starting a queue from track IDs or a recipe: `startQueue`.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use super::{Recipe, refused, resolve, small};
use crate::api::player::Players;
use crate::api::realtime::{Hub, ServerMessage};
use crate::api::scope::Caller;
use crate::api::search::Indexes;
use crate::api::{Id, Problem, Uuid, extract, plays};
use crate::db::{Database, reaches};
use crate::queue::{KeepError, NewQueue, Queues, list_hash};

/// The spec's `StartQueue`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Start {
    queue_id: Uuid,
    /// Kept raw until its size is checked, so an oversized one is never built as a tree.
    context: Box<RawValue>,
    source: NewSource,
    /// The index of the track that plays first. For a recipe, the index in the view the device
    /// showed, which the server uses if its list holds `start_track_id` there.
    start: u64,
    /// With a recipe, the track that plays first.
    start_track_id: Option<String>,
}

/// A new source: the tracks as the device resolved them, or the recipe for the server to
/// resolve. Exactly one of the two.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewSource {
    key: Uuid,
    library_id: String,
    /// Read straight into IDs, eight bytes each, so the body is the largest thing a start holds.
    tracks: Option<Vec<Id>>,
    /// Kept raw until its size is checked, as `context` is.
    recipe: Option<Box<RawValue>>,
}

/// The spec's `StartedQueue`: the list the server holds for the start, which the device attaches
/// its own tracks to under the hash, so it never downloads them back.
#[derive(Serialize)]
pub struct Started {
    list: String,
    length: usize,
}

/// Starts a queue that plays the request's tracks, or the list its recipe resolves to, from
/// `start`, parking the one it replaces (kept if it was adjusted, `requirements/queue.md` §6),
/// and tells the account's connections. Refused, `422`, unless every track is in the library
/// and the caller reaches the library, whether or not what is refused exists, so no one learns
/// what a library they cannot reach holds (`requirements/users.md` §10). A recipe's list starts
/// at `start` if it holds `startTrackId` there, and otherwise at that track's first place, and a
/// list without it is refused. A request for a queue the account already has, active or
/// kept, by `queueId`, changes nothing and is answered as the first was, so a device may send it
/// again.
pub async fn start(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    State(players): State<Arc<Players>>,
    State(hub): State<Arc<Hub>>,
    State(indexes): State<Arc<Indexes>>,
    caller: Caller,
    extract::Json(request): extract::Json<Start>,
) -> Result<Json<Started>, Problem> {
    let context = context(&request.context)?;
    let NewSource { key, library_id, tracks, recipe } = request.source;
    let library = Id::canonical(&library_id).ok_or_else(refused)?;
    let (ids, recipe, at) = match (tracks, recipe, request.start_track_id) {
        (Some(tracks), None, None) => {
            let ids: Vec<u64> = tracks.into_iter().map(|Id(id)| id.cast_unsigned()).collect();
            check_tracks(&db, &caller, library, &ids).await?;
            (ids, None, request.start)
        }
        (None, Some(recipe), Some(track)) => {
            let recipe: Recipe = small(&recipe, "A recipe")?;
            let track = Id::canonical(&track).map_or(u64::MAX, |Id(id)| id.cast_unsigned());
            let ids = resolve(&db, &indexes, &caller, library, &recipe).await?;
            let at = start_at(&ids, request.start, track)
                .ok_or_else(|| Problem::invalid("startTrackId is not in the recipe's tracks."))?;
            (ids, Some(recipe.stored()), at)
        }
        _ => {
            return Err(Problem::invalid(
                "A source is either tracks, or a recipe with startTrackId.",
            ));
        }
    };
    let length = ids.len();
    let tracks: Arc<[u64]> = ids.into();

    let user = caller.user;
    let queue_id = request.queue_id;
    let new = NewQueue {
        queue_id: queue_id.clone(),
        context,
        key: key.as_str().to_owned(),
        library_id: library.0,
        tracks: tracks.clone(),
        recipe,
        at,
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

/// Where `track` plays first in `list`: `start` if it holds `track` there, and otherwise the
/// first place that does, if any.
fn start_at(list: &[u64], start: u64, track: u64) -> Option<u64> {
    let there = usize::try_from(start).ok().and_then(|start| list.get(start));
    if there == Some(&track) {
        return Some(start);
    }
    list.iter().position(|&id| id == track).map(|at| at as u64)
}

/// A start's context, checked as a play's is and kept as sent. Its size is checked before it is
/// parsed.
fn context(raw: &RawValue) -> Result<Value, Problem> {
    const FIELD: &str = "A queue's context";
    let value = small(raw, FIELD)?;
    plays::json_text(&value, FIELD, true)?;
    Ok(value)
}

/// Checks that every one of `ids` is in `library`, which the caller must reach. Refused, `422`,
/// as a start is.
async fn check_tracks(
    db: &Database,
    caller: &Caller,
    library: Id,
    ids: &[u64],
) -> Result<(), Problem> {
    let mut distinct = ids.to_vec();
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
    if found == distinct.len() { Ok(()) } else { Err(refused()) }
}
