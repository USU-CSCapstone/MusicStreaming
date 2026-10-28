//! Editing the queue: `editQueue`.
//!
//! Devices send edits that name items, so the server applies them in the order they arrive
//! (`design/queue.md`). An edit is applied through the shared core under the account's lock,
//! which bumps the version by one and sends the edit, as applied, to every connection of the
//! account. The server keeps the `editId`s of the latest edits, so one sent again is applied
//! once and answered the same.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_core::{Anchor, Edit, QueueError, Source};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::start::{NewSource, refused, resolve};
use super::{Account, Queues};
use crate::api::realtime::{Hub, ServerMessage};
use crate::api::scope::Caller;
use crate::api::search::Indexes;
use crate::api::{Code, Id, Problem, Uuid, extract};
use crate::db::Database;

/// How many of an account's latest edits are kept to answer one sent again, as many as it keeps
/// player commands.
const REMEMBERED: usize = 32;

/// The spec's `QueueEditRequest`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditRequest {
    edit_id: Uuid,
    queue_id: Uuid,
    /// The core's `Edit`, except that an `add`'s source may be a `query`; so it is read in two
    /// steps.
    edit: Value,
}

/// The answer to an applied edit.
#[derive(Serialize)]
pub struct Applied {
    /// The queue's version once the edit was applied.
    version: u32,
}

/// An `add` as sent, whose source the server may have to resolve.
#[derive(Deserialize)]
struct Add {
    source: NewSource,
    before: Anchor,
}

/// `editQueue`: applies the edit to the caller's queue and answers its new version. An `add`
/// of `tracks` is checked as a start's are, and one of a `query` is run by the server, so
/// every device gets the same tracks; an `add` that is not sent as an object with `type`
/// `add` is refused, so none skips that check. `404` without a queue, `409` if `queueId` is not the
/// queue the caller has or the edit's anchor is gone, `422` for an edit the core refuses.
pub async fn edit(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    State(indexes): State<Arc<Indexes>>,
    State(hub): State<Arc<Hub>>,
    caller: Caller,
    extract::Json(request): extract::Json<EditRequest>,
) -> Result<Json<Applied>, Problem> {
    let invalid = |error: serde_json::Error| Problem::invalid(error.to_string());
    let adds = request.edit.get("type").and_then(Value::as_str) == Some("add");
    let edit = if adds {
        let Add { source, before } = serde_json::from_value(request.edit).map_err(invalid)?;
        let NewSource { key, library_id, tracks, query } = source;
        let library = Id::canonical(&library_id).ok_or_else(refused)?;
        let tracks = resolve(&db, &indexes, &caller, library, tracks, query).await?;
        let source = Source { key: key.as_str().to_owned(), library_id, tracks };
        Edit::Add { source, before }
    } else {
        // The core also reads an `add` from a JSON array, which has not been checked above.
        match serde_json::from_value(request.edit).map_err(invalid)? {
            Edit::Add { .. } => return Err(Problem::invalid("An add must be an object.")),
            edit => edit,
        }
    };

    // A queue of hundreds of thousands of tracks takes a while to edit and to send.
    let (user, queue_id, edit_id) = (caller.user, request.queue_id, request.edit_id);
    let version =
        tokio::task::spawn_blocking(move || queues.apply(&hub, user, &queue_id, edit_id, edit))
            .await
            .map_err(|_| Problem::new(Code::Internal))??;
    Ok(Json(Applied { version }))
}

impl Queues {
    /// Applies `edit`, which `edit_id` names, to `user`'s queue `queue_id`, and answers the
    /// version it made, sending it to all of their connections. An `edit_id` among the latest
    /// is not applied again, and answers the version it made. A refused edit changes nothing.
    pub(super) fn apply(
        &self,
        hub: &Hub,
        user: i64,
        queue_id: &Uuid,
        edit_id: Uuid,
        edit: Edit,
    ) -> Result<u32, Problem> {
        let mut accounts = self.lock();
        let account = accounts.get_mut(&user).ok_or_else(Problem::not_found)?;
        if account.queue_id != *queue_id {
            let detail = "The account's queue is not the one this edit is for.";
            return Err(Problem::new(Code::QueueChanged).detail(detail));
        }
        if let Some((_, version)) = account.done.iter().find(|(id, _)| *id == edit_id) {
            return Ok(*version);
        }
        account.queue.apply(&edit).map_err(|error| match error {
            QueueError::AnchorGone => Problem::new(Code::AnchorGone).detail(error.to_string()),
            _ => Problem::invalid(error.to_string()),
        })?;
        account.version += 1;
        account.unsaved = true;
        remember(account, edit_id.clone());
        // Under the lock, so connections hear the edits in the order of their versions.
        let version = account.version;
        let message =
            ServerMessage::QueueEdit { queue_id: queue_id.clone(), version, edit_id, edit };
        hub.broadcast(user, &message);
        Ok(version)
    }
}

/// Notes that `account`'s latest edit, `edit_id`, made its current version.
fn remember(account: &mut Account, edit_id: Uuid) {
    if account.done.len() == REMEMBERED {
        account.done.pop_front();
    }
    account.done.push_back((edit_id, account.version));
}
