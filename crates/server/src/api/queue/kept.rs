//! The account's kept queues: `listKeptQueues`, `getKeptQueue`, `restoreQueue` and
//! `deleteKeptQueue`
//! (`design/queue.md` §8, §9).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rusqlite::types::Value as Sql;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::extract::{Path, Query};
use crate::api::player::Players;
use crate::api::problem::Code;
use crate::api::realtime::{Hub, ServerMessage};
use crate::api::scope::Caller;
use crate::api::time::serialize_rfc3339;
use crate::api::{Id, Problem, Uuid, cursor, page};
use crate::db::Database;
use crate::queue::{Current, Entry, KeepError, Key, Queues, names};

/// What names a list's cursor, so one from another list is refused.
const CURSOR: &str = "queues";

/// The spec's query for `listKeptQueues`.
#[derive(Deserialize)]
pub struct ListQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}

/// The spec's `KeptQueue`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    queue_id: Uuid,
    context: Value,
    #[serde(serialize_with = "serialize_rfc3339")]
    last_active: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<Playing>,
    up_next: u64,
    history: u64,
}

/// The spec's `KeptQueue.current`: `track_id` is `null` where the track can't be read.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Playing {
    library_id: String,
    track_id: Option<Id>,
}

/// The spec's `KeptQueuePage`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeptPage {
    items: Vec<Kept>,
    next_cursor: Option<String>,
    total: u64,
}

/// The caller's kept queues, most recently active first, a page at a time (`limit` as other
/// lists have it). Includes an adjusted queue a start has just parked and the save has not
/// written yet. The first page carries an `ETag` over the whole kept set (`Queues::kept_tag`),
/// and is `304` when `If-None-Match` names it, so a device whose list is current reads nothing
/// more. `422` for a cursor that did not come from this list.
pub async fn list(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    caller: Caller,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Response, Problem> {
    let limit = page::limit(query.limit)?;
    let after = query.cursor.map(|text| after(&text)).transpose()?;
    // Taken before the page, so a change made meanwhile shows as a tag that no longer matches.
    let tag = match after {
        None => Some(queues.kept_tag(&db, caller.user, limit).await?),
        Some(_) => None,
    };
    let known = headers.get(header::IF_NONE_MATCH).and_then(|value| value.to_str().ok());
    if let (Some(tag), Some(known)) = (&tag, known)
        && names(known, tag)
    {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, tag.as_str())]).into_response());
    }
    let listed = queues.kept(&db, caller.user, after, limit).await?;
    let items = listed.entries.into_iter().map(kept).collect();
    let next_cursor =
        listed.next.map(|(at, id)| cursor::encode(CURSOR, &[Sql::Integer(at), Sql::Text(id)]));
    let page = Json(KeptPage { items, next_cursor, total: listed.total });
    Ok(match tag {
        Some(tag) => ([(header::ETAG, tag)], page).into_response(),
        None => page.into_response(),
    })
}

/// The key a cursor continues after.
fn after(text: &str) -> Result<Key, Problem> {
    match cursor::decode(text, CURSOR, 2)?.as_slice() {
        [Sql::Integer(at), Sql::Text(id)] => Ok((*at, id.clone())),
        _ => Err(Problem::invalid("cursor is not valid for this list")),
    }
}

/// The wire form of `entry`.
fn kept(entry: Entry) -> Kept {
    let playing = |current: Current| Playing {
        library_id: current.library_id,
        track_id: current.track_id.map(|track| Id(track.cast_signed())),
    };
    Kept {
        queue_id: entry.queue_id,
        context: entry.context,
        last_active: entry.last_active,
        current: entry.current.map(playing),
        up_next: entry.up_next,
        history: entry.history,
    }
}

/// The caller's kept queue `queue_id` as a restore would make it active, with where its playback
/// was left, without restoring it, so a device can restore it while offline. `404` for a queue
/// that is not kept, whoever's it is, the active one included.
pub async fn get_kept(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    caller: Caller,
    Path(queue_id): Path<Uuid>,
) -> Result<Response, Problem> {
    let body =
        queues.kept_snapshot(&db, caller.user, queue_id).await?.ok_or_else(Problem::not_found)?;
    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}

/// Makes the caller's kept queue `queue_id` the active one, parking the queue it replaces as a
/// start does, and answers as `getQueue` does, with the queue's `position`: where its playback
/// was left, as [`get_kept`] answers it too. The player state is left alone, as a start leaves
/// it. `404` for a queue that is not kept, whoever's it is, `409 queue_active` for the active
/// one. Sends the account `queueReplaced` and `queuesChanged`.
pub async fn restore(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    State(players): State<Arc<Players>>,
    State(hub): State<Arc<Hub>>,
    caller: Caller,
    Path(queue_id): Path<Uuid>,
) -> Result<Response, Problem> {
    let user = caller.user;
    // Under the queues' lock, so lock order is queues before player state.
    let place = move || players.position(user);
    let position = queues.restore(&db, user, queue_id.clone(), place).await.map_err(problem)?;
    hub.broadcast(user, &ServerMessage::QueueReplaced { queue_id: queue_id.clone() });
    hub.broadcast(user, &ServerMessage::QueuesChanged);
    let position = position.as_ref().map(|at| (&queue_id, at));
    let (etag, body) = queues.snapshot(user, None, position).ok_or_else(Problem::not_found)?;
    // No `known` tag was given, so the body is always there.
    let body = body.ok_or_else(Problem::not_found)?;
    Ok(([(header::CONTENT_TYPE, "application/json"), (header::ETAG, &etag)], body).into_response())
}

/// Deletes the caller's kept queue `queue_id`, and sends the account `queuesChanged`. `204`.
/// `404` for a queue that is not kept, whoever's it is, `409 queue_active` for the active one.
pub async fn delete(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    State(hub): State<Arc<Hub>>,
    caller: Caller,
    Path(queue_id): Path<Uuid>,
) -> Result<StatusCode, Problem> {
    queues.delete(&db, caller.user, queue_id).await.map_err(problem)?;
    hub.broadcast(caller.user, &ServerMessage::QueuesChanged);
    Ok(StatusCode::NO_CONTENT)
}

/// The answer for a restore or delete that did nothing.
fn problem(error: KeepError) -> Problem {
    match error {
        KeepError::NotFound => Problem::not_found(),
        KeepError::Active => Problem::new(Code::QueueActive)
            .detail("The active queue is neither restored nor deleted."),
        KeepError::Db(error) => error.into(),
        KeepError::Exists | KeepError::NotInList(_) => unreachable!("only starts"),
    }
}
