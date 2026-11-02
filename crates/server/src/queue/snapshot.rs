//! The snapshot `GET /me/queue` answers: the active queue's state, without tracks.

use jewelcase_core::QueueState;
use serde::Serialize;
use serde_json::Value;

use super::store::Queues;
use crate::api::Uuid;

/// The spec's `QueueSnapshot`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot<'a> {
    queue_id: &'a Uuid,
    version: u32,
    context: &'a Value,
    state: &'a QueueState,
}

/// Whether the `If-None-Match` value `known` matches `etag`, compared weakly as RFC 9110 §13.1.2
/// has it: `*` matches, and so does any tag of a list, weak or strong, with the same text.
fn names(known: &str, etag: &str) -> bool {
    known.split(',').map(str::trim).any(|tag| tag == "*" || tag.trim_start_matches("W/") == etag)
}

impl Queues {
    /// `user`'s snapshot, or `None` if they have no queue: its `ETag`, and its JSON unless
    /// `known` (an `If-None-Match` value) names it, which is checked before anything is
    /// serialized.
    ///
    /// The tag is `"<boot>.<queueId>.<version>"`, so it differs for every queue and every
    /// change, and for every start of the server.
    pub fn snapshot(&self, user: i64, known: Option<&str>) -> Option<(String, Option<Vec<u8>>)> {
        // Only copies are made under the lock, so serializing does not hold up other accounts.
        let (etag, queue_id, version, context, state) = {
            let accounts = self.lock();
            let account = accounts.get(&user)?;
            let etag =
                format!("\"{}.{}.{}\"", self.boot, account.queue_id.as_str(), account.version);
            if known.is_some_and(|known| names(known, &etag)) {
                return Some((etag, None));
            }
            (
                etag,
                account.queue_id.clone(),
                account.version,
                account.context.clone(),
                account.state.clone(),
            )
        };
        let snapshot = Snapshot { queue_id: &queue_id, version, context: &context, state: &state };
        // A state and a context are plain data, so this cannot fail.
        let body = serde_json::to_vec(&snapshot).ok()?;
        Some((etag, Some(body)))
    }
}
