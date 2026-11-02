//! Editing the queue: `editQueue`, and what it sends the account's connections.
//!
//! Devices send edits that name items, so the server applies them in the order they arrive
//! (`design/queue.md` §9). An edit is applied through the shared core under the account's lock,
//! which bumps the version by one and sends the edit, as applied, to every connection of the
//! account. The server keeps the `editId`s of the latest edits, so one sent again is applied
//! once and answered the same.

use std::fmt;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_core::{Anchor, Edit, ItemId, PlayRef, QueueError, Source};
use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};
use serde_json::value::RawValue;

use super::small;
use super::start::{NewSource, Resolved};
use crate::api::realtime::{Hub, ServerMessage};
use crate::api::scope::Caller;
use crate::api::search::Indexes;
use crate::api::{Code, Problem, Uuid, extract};
use crate::db::Database;
use crate::queue::{Added, Applied, EditError, Queues, list_hash};

/// The spec's `EditQueue`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditRequest {
    edit_id: Uuid,
    queue_id: Uuid,
    /// The core's `Edit`, except that an `add`'s source is a [`NewSource`]; so it is read in
    /// two steps, from the raw text, never as a tree.
    edit: Box<RawValue>,
}

/// What a refusal calls an anchor that is too large.
const ANCHOR: &str = "An anchor";

/// An edit's `type`, read without building the rest of it.
#[derive(Deserialize)]
struct Kind {
    #[serde(rename = "type")]
    kind: String,
}

/// A `remove` as sent.
#[derive(Deserialize)]
struct Remove {
    items: Vec<SentItem>,
}

/// A `move` as sent.
#[derive(Deserialize)]
struct Move {
    items: Vec<SentItem>,
    /// An [`Anchor`], kept raw until its size is checked.
    before: Box<RawValue>,
}

/// A `removePlays` as sent.
#[derive(Deserialize)]
struct RemovePlays {
    plays: Vec<SentPlay>,
}

/// A play of History as an edit names it, with a [`SentItem`].
#[derive(Deserialize)]
struct SentPlay {
    play: u64,
    item: SentItem,
}

/// An item an edit names, `<key>.<position>`, whose key must be a UUID; it is kept lowercase,
/// as keys are. Read from the text as it is parsed, so an item held costs about what its JSON
/// does.
struct SentItem(ItemId);

impl<'de> Deserialize<'de> for SentItem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<SentItem, D::Error> {
        deserializer.deserialize_str(ItemVisitor)
    }
}

/// Reads a [`SentItem`] from a string.
struct ItemVisitor;

impl Visitor<'_> for ItemVisitor {
    type Value = SentItem;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an item, <key>.<position>, whose key is a UUID")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<SentItem, E> {
        let invalid = || E::custom("an item is <key>.<position>, and its key a UUID");
        let (key, pos) = text.rsplit_once('.').ok_or_else(invalid)?;
        let key = Uuid::try_from(key.to_owned()).map_err(|_| invalid())?;
        let pos = pos.parse().map_err(|_| invalid())?;
        Ok(SentItem(ItemId { key: key.as_str().to_owned(), pos }))
    }
}

/// The core's items of `sent`.
fn items(sent: Vec<SentItem>) -> Vec<ItemId> {
    sent.into_iter().map(|SentItem(item)| item).collect()
}

/// An `add` as sent, whose source the server resolves into a list of its own.
#[derive(Deserialize)]
struct Add {
    source: NewSource,
    /// An [`Anchor`], kept raw until its size is checked.
    before: Box<RawValue>,
}

/// `editQueue`: applies the edit to the caller's active queue and answers the version it made,
/// and for an add the source as registered. An add of `tracks` is checked as a start's are, and
/// one of a `recipe` is resolved by the server, so every device gets the same tracks; whatever
/// `list` or `length` a client sends for it is ignored, and the server holds the list it made.
/// Keys must be UUIDs, and so must the keys of the items a `remove`, `move` or `removePlays`
/// names. `404` without a queue, `409` if `queueId` is not the active queue or an anchor or the
/// play a `rewind` names is gone, `422` for tracks out of reach or not in the library, a key
/// that is not a UUID or already in use, an anchor or recipe over 4 KiB, any edit but those
/// four over 4 KiB, or an edit the core refuses.
pub async fn edit(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    State(indexes): State<Arc<Indexes>>,
    State(hub): State<Arc<Hub>>,
    caller: Caller,
    extract::Json(request): extract::Json<EditRequest>,
) -> Result<Json<Applied>, Problem> {
    let invalid = |error: serde_json::Error| Problem::invalid(error.to_string());
    let EditRequest { edit_id, queue_id, edit: raw } = request;
    let text = raw.get();
    // Only an edit whose `type` is `add` can read as an add, so no other path skips the checks.
    let Kind { kind } = serde_json::from_str(text).map_err(invalid)?;
    let (edit, added) = if kind == "add" {
        let Add { source, before } = serde_json::from_str(text).map_err(invalid)?;
        let before: Anchor = small(&before, ANCHOR)?;
        // A retry of an applied add is answered as the first was, even if its source is gone.
        if let Some(applied) = queues.answered(caller.user, &queue_id, &edit_id) {
            return Ok(Json(applied));
        }
        let Resolved { key, library, ids, recipe } = source.resolve(&db, &indexes, &caller).await?;
        let tracks: Arc<[u64]> = ids.into();
        let hash = list_hash(&tracks);
        let mut source =
            Source::new(key.as_str(), library.0.to_string(), tracks.len() as u64, recipe);
        source.hash = Some(hash.clone());
        (Edit::Add { source, before }, Some(Added { hash, library_id: library.0, tracks }))
    } else if kind == "remove" {
        // The edits that name many items are read as the server's own types: the core's
        // tagged `Edit` would be buffered whole before it is read.
        let Remove { items: sent } = serde_json::from_str(text).map_err(invalid)?;
        (Edit::Remove { items: items(sent) }, None)
    } else if kind == "move" {
        let Move { items: sent, before } = serde_json::from_str(text).map_err(invalid)?;
        let before: Anchor = small(&before, ANCHOR)?;
        (Edit::Move { items: items(sent), before }, None)
    } else if kind == "removePlays" {
        let RemovePlays { plays } = serde_json::from_str(text).map_err(invalid)?;
        let plays = plays
            .into_iter()
            .map(|SentPlay { play, item: SentItem(item) }| PlayRef { play, item })
            .collect();
        (Edit::RemovePlays { plays }, None)
    } else {
        let mut edit: Edit = small(&raw, "An edit of this type")?;
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
    // The edit is read, so its text need not wait for it to apply.
    drop(raw);

    let user = caller.user;
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
        QueueError::PlayGone => Problem::new(Code::PlayGone),
        QueueError::DuplicateKey => Problem::new(Code::DuplicateKey),
        _ => Problem::invalid(error.to_string()),
    }
    .detail(error.to_string())
}
