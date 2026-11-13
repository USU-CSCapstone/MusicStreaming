//! Applying edits to the active queue (`design/queue.md` §9).
//!
//! Edits name items, never positions, so the server applies them in the order they arrive and
//! no version can be refused. Each applies through the core under the lock, bumps the version
//! and is announced under the same lock, so every connection hears the edits in version order.

use std::sync::Arc;

use jewelcase_core::{Edit, QueueError, Source};
use serde::Serialize;

use super::store::{Held, Queues};
use crate::api::Uuid;

/// Why an edit was not applied. Nothing changed.
#[derive(Debug, PartialEq, Eq)]
pub enum EditError {
    /// The account has no queue.
    NoQueue,
    /// The edit names a queue that is not the active one: replaced, or kept.
    Changed,
    /// The core refused the edit.
    Refused(QueueError),
}

/// A list an add brings, which the server holds from the moment the edit applies.
pub struct Added {
    /// The list's hash, which the add's source carries.
    pub hash: String,
    /// The library every track belongs to.
    pub library_id: i64,
    /// The track IDs.
    pub tracks: Arc<[u64]>,
}

/// An edit applied, which is the spec's `AppliedEdit`: the queue's version once it was, and for
/// an add the source registered, whose `list` the sender attaches its own tracks to.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Applied {
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

impl Queues {
    /// Applies `edit` to `user`'s queue `queue_id`, holding the list of an add (`added`, whose
    /// hash the edit's source must carry), and marks the account changed. `announce` is called
    /// under the lock with the new version and the edit, to send it to the account's
    /// connections. An edit sent again is not recognised here: the idempotency layer answers it
    /// before it gets this far (`api/idempotency.rs`).
    pub fn apply(
        &self,
        user: i64,
        queue_id: &Uuid,
        edit: Edit,
        added: Option<Added>,
        announce: impl FnOnce(u32, Edit),
    ) -> Result<Applied, EditError> {
        let mut accounts = self.lock();
        let account = accounts.get_mut(&user).ok_or(EditError::NoQueue)?;
        if account.queue_id != *queue_id {
            return Err(EditError::Changed);
        }
        account.state.apply(&edit).map_err(EditError::Refused)?;
        if let Some(Added { hash, library_id, tracks }) = added.filter(|a| !a.tracks.is_empty()) {
            account.lists.insert(hash, Held { library_id, tracks });
        }
        account.version += 1;
        account.changed = true;
        let source = match &edit {
            Edit::Add { source, .. } => Some(source.clone()),
            _ => None,
        };
        let version = account.version;
        announce(version, edit);
        Ok(Applied { version, source })
    }
}
