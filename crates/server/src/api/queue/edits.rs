//! Editing the queue: `editQueue`, and what it sends the account's connections.
//!
//! Devices send edits that name items, so the server applies them in the order they arrive
//! (`design/queue.md` §9). An edit is applied through the shared core under the account's lock,
//! which bumps the version by one and sends the edit, as applied, to every connection of the
//! account. The server keeps the `editId`s of the latest edits, so one sent again is applied
//! once and answered the same.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_core::{Anchor, Edit, QueueError, Source};
use serde::Deserialize;
use serde_json::Value;

use super::start::{NewSource, check_tracks};
use super::{Recipe, refused, resolve, small};
use crate::api::realtime::{Hub, ServerMessage};
use crate::api::scope::Caller;
use crate::api::search::Indexes;
use crate::api::{Code, Id, Problem, Uuid, extract};
use crate::db::Database;
use crate::queue::{Added, Applied, EditError, Queues, list_hash};

/// The spec's `EditQueue`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditRequest {
    edit_id: Uuid,
    queue_id: Uuid,
    /// The core's `Edit`, except that an `add`'s source is a [`NewSource`]; so it is read in
    /// two steps.
    edit: Value,
}

/// An `add` as sent, whose source the server resolves into a list of its own.
#[derive(Deserialize)]
struct Add {
    source: NewSource,
    before: Anchor,
}

/// `editQueue`: applies the edit to the caller's active queue and answers the version it made,
/// and for an add the source as registered. An add of `tracks` is checked as a start's are, and
/// one of a `recipe` is resolved by the server, so every device gets the same tracks; whatever
/// `list` or `length` a client sends for it is ignored, and the server holds the list it made.
/// Keys must be UUIDs. `404` without a queue, `409` if `queueId` is not the active queue or an
/// anchor is gone, `422` for tracks out of reach or not in the library, a key that is not a UUID
/// or already in use, or an edit the core refuses.
pub async fn edit(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    State(indexes): State<Arc<Indexes>>,
    State(hub): State<Arc<Hub>>,
    caller: Caller,
    extract::Json(request): extract::Json<EditRequest>,
) -> Result<Json<Applied>, Problem> {
    let invalid = |error: serde_json::Error| Problem::invalid(error.to_string());
    // Only a value whose `type` is `add` can read as an add, so no other path skips the checks.
    let adds = request.edit.get("type").and_then(Value::as_str) == Some("add");
    let (edit, added) = if adds {
        let Add { source, before } = serde_json::from_value(request.edit).map_err(invalid)?;
        let NewSource { key, library_id, tracks, recipe } = source;
        let library = Id::canonical(&library_id).ok_or_else(refused)?;
        let (ids, recipe) = match (tracks, recipe) {
            (Some(tracks), None) => {
                let ids: Vec<u64> = tracks.into_iter().map(|Id(id)| id.cast_unsigned()).collect();
                check_tracks(&db, &caller, library, &ids).await?;
                (ids, None)
            }
            (None, Some(recipe)) => {
                let recipe: Recipe = small(&recipe, "A recipe")?;
                (resolve(&db, &indexes, &caller, library, &recipe).await?, Some(recipe.stored()))
            }
            _ => return Err(Problem::invalid("A source is either tracks or a recipe.")),
        };
        let tracks: Arc<[u64]> = ids.into();
        let hash = list_hash(&tracks);
        let mut source =
            Source::new(key.as_str(), library.0.to_string(), tracks.len() as u64, recipe);
        source.hash = Some(hash.clone());
        (Edit::Add { source, before }, Some(Added { hash, library_id: library.0, tracks }))
    } else {
        let mut edit: Edit = serde_json::from_value(request.edit).map_err(invalid)?;
        // Keys are kept in lowercase, as an add's are.
        if let Edit::Previous { key } | Edit::Requeue { key, .. } | Edit::Rewind { key, .. } =
            &mut edit
        {
            let uuid = Uuid::try_from(std::mem::take(key))
                .map_err(|_| Problem::invalid("A key must be a UUID."))?;
            *key = uuid.as_str().to_owned();
        }
        (edit, None)
    };

    let (user, queue_id, edit_id) = (caller.user, request.queue_id, request.edit_id);
    let announced = (queue_id.clone(), edit_id.clone());
    let announce = |version, edit| {
        let (queue_id, edit_id) = announced;
        hub.broadcast(user, &ServerMessage::QueueEdit { queue_id, version, edit_id, edit });
    };
    let applied = queues.apply(user, &queue_id, edit_id, edit, added, announce).map_err(
        |error| match error {
            EditError::NoQueue => Problem::not_found(),
            EditError::Changed => Problem::new(Code::QueueChanged)
                .detail("The account's active queue is not the one this edit is for."),
            EditError::Refused(error) => refusal(error),
        },
    )?;
    Ok(Json(applied))
}

/// The problem for an edit the core refused, with the core's own code where the spec names one.
fn refusal(error: QueueError) -> Problem {
    match error {
        QueueError::AnchorGone => Problem::new(Code::AnchorGone),
        QueueError::DuplicateKey => Problem::new(Code::DuplicateKey),
        _ => Problem::invalid(error.to_string()),
    }
    .detail(error.to_string())
}
