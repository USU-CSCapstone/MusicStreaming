//! Listing an account's kept queues (`design/queue.md` §8, §9).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use jewelcase_core::QueueState;
use rusqlite::{Connection, params};
use serde_json::Value;

use super::read::{Slot, read_tracks, slot};
use super::store::Queues;
use crate::api::Uuid;
use crate::db::{Database, DbError, now_ms};

/// Where a queue sits in the listing: its `last_active`, then its ID, both newest first. A queue
/// parked and not yet saved has no stored time, so it sorts before every stored one.
pub type Key = (i64, String);

/// The sort time of a parked queue not yet saved.
const UNSAVED: i64 = i64::MAX;

/// One kept queue as listed: what was playing, and how many rows each tab holds.
pub struct Entry {
    pub queue_id: Uuid,
    pub context: Value,
    /// When the queue was parked, in ms; the time of the listing for one not yet saved.
    pub last_active: i64,
    pub current: Option<Current>,
    pub up_next: u64,
    pub history: u64,
}

/// What a kept queue was playing: the library, and the track unless it can't be read.
pub struct Current {
    pub library_id: String,
    pub track_id: Option<u64>,
}

/// A page of the listing, and the key to continue after if there is more.
pub struct Listed {
    pub entries: Vec<Entry>,
    pub next: Option<Key>,
}

/// What an account has parked in memory.
#[derive(Default)]
struct Parked {
    /// The adjusted queues, the others being due for deletion.
    drafts: Vec<Draft>,
    /// The IDs the stored rows must not repeat: every parked queue, and the active one.
    hidden: HashSet<String>,
    /// The account's held lists.
    held: HashMap<String, Arc<[u64]>>,
}

/// A queue's entry before its current track is read.
struct Draft {
    queue_id: Uuid,
    context: Value,
    last_active: i64,
    current: Option<Slot>,
    up_next: u64,
    history: u64,
}

impl Draft {
    /// What `state` says about the queue, which needs no lists.
    fn new(queue_id: Uuid, context: Value, last_active: i64, state: &QueueState) -> Draft {
        let current = state.current_row().and_then(|row| slot(state, row, None));
        Draft {
            queue_id,
            context,
            last_active,
            current,
            up_next: state.up_next_len(),
            history: state.history_len(),
        }
    }
}

impl Queues {
    /// Up to `limit` of `user`'s kept queues after `after` (from the start if `None`), most
    /// recently parked first, and where the next page continues, if there is one.
    ///
    /// Queues parked in memory and not yet saved come first if they were adjusted, the others
    /// being due for deletion; then the stored ones, so a list fetched just after a start shows
    /// the queue it kept. Memory is read before the database, so a queue saved between the two
    /// is seen twice and listed once. A stored row that can't be read is skipped and logged.
    /// Each queue's current track is read as the rows endpoint reads it, after the lock.
    pub async fn kept(
        &self,
        db: &Database,
        user: i64,
        after: Option<Key>,
        limit: u32,
    ) -> Result<Listed, DbError> {
        let Parked { drafts, hidden, held } = self.parked(user);
        let limit = limit as usize;
        let mut candidates: Vec<(Key, Option<Draft>)> = drafts
            .into_iter()
            .map(|draft| ((UNSAVED, draft.queue_id.as_str().to_owned()), Some(draft)))
            .filter(|(key, _)| after.as_ref().is_none_or(|after| key < after))
            .collect();
        candidates.sort_by(|a, b| b.0.cmp(&a.0));
        let stored = db.read(move |conn| stored(conn, user, after, limit + 1)).await?;
        candidates.extend(stored.into_iter().filter(|(key, _)| !hidden.contains(&key.1)));

        let more = candidates.len() > limit;
        candidates.truncate(limit);
        let next = candidates.last().filter(|_| more).map(|(key, _)| key.clone());
        let drafts: Vec<Draft> = candidates.into_iter().filter_map(|(_, draft)| draft).collect();
        let tracks =
            read_tracks(db, user, drafts.iter().filter_map(|d| d.current.as_ref()), &held).await?;
        let mut tracks = tracks.into_iter();
        let entries = drafts
            .into_iter()
            .map(|draft| Entry {
                current: draft.current.map(|slot| Current {
                    library_id: slot.library_id,
                    track_id: tracks.next().flatten(),
                }),
                queue_id: draft.queue_id,
                context: draft.context,
                last_active: draft.last_active,
                up_next: draft.up_next,
                history: draft.history,
            })
            .collect();
        Ok(Listed { entries, next })
    }

    /// Under the lock: what the account has parked and not yet saved, and its held lists.
    fn parked(&self, user: i64) -> Parked {
        let accounts = self.lock();
        let Some(account) = accounts.get(&user) else { return Default::default() };
        let now = now_ms();
        let drafts = account
            .parked
            .iter()
            .filter(|queue| queue.state.adjusted)
            .map(|q| Draft::new(q.queue_id.clone(), q.context.clone(), now, &q.state))
            .collect();
        let hidden = account
            .parked
            .iter()
            .map(|queue| queue.queue_id.as_str().to_owned())
            .chain([account.queue_id.as_str().to_owned()])
            .collect();
        let held = account.lists.iter().map(|(h, l)| (h.clone(), l.tracks.clone())).collect();
        Parked { drafts, hidden, held }
    }
}

/// Up to `limit` of `user`'s stored kept queues after `after`, newest first, keyed for
/// paging. A row that can't be read has no draft.
fn stored(
    conn: &Connection,
    user: i64,
    after: Option<Key>,
    limit: usize,
) -> rusqlite::Result<Vec<(Key, Option<Draft>)>> {
    let (at, id) = after.unzip();
    let mut select = conn.prepare_cached(
        "SELECT queue_id, context, state, last_active FROM queues \
         WHERE user_id = ?1 AND active = 0 AND (?2 IS NULL OR (last_active, queue_id) < (?2, ?3)) \
         ORDER BY last_active DESC, queue_id DESC LIMIT ?4",
    )?;
    let rows = select.query_map(
        params![user, at, id, i64::try_from(limit).unwrap_or(i64::MAX)],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        },
    )?;
    let mut found = Vec::new();
    for row in rows {
        let (queue_id, context, state, last_active) = row?;
        let key = (last_active, queue_id.clone());
        let read = (
            Uuid::try_from(queue_id),
            serde_json::from_str::<Value>(&context),
            serde_json::from_str::<QueueState>(&state),
        );
        let draft = if let (Ok(id), Ok(context), Ok(state)) = read {
            Some(Draft::new(id, context, last_active, &state))
        } else {
            tracing::warn!(user, "cannot read a kept queue");
            None
        };
        found.push((key, draft));
    }
    Ok(found)
}
