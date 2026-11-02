//! The background save of each account's queues and lists (`design/queue.md` §8).
//!
//! Every second, each changed account is written in a savepoint of its own: the queues parked
//! since the last save, kept or deleted, then the lists the states reference and the database
//! lacks, then the active state, then the removal of the account's lists no queue uses. All of
//! it runs on the database's writer, which takes what it needs from memory under the lock, so
//! nothing else that writes an account's queues can meet a half-done save.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{Transaction, params};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::hash::list_bytes;
use super::store::{Account, Held, Queue, Queues};
use crate::db::Database;
use serde_json::Value;

/// How long a change may go unsaved: as much of a queue as a crash can lose anyone.
const SAVE_EVERY: Duration = Duration::from_secs(1);

/// What one account's save took from memory.
pub(super) struct Pending {
    queue: QueueRow,
    /// The queues parked since the last save, oldest first.
    parked: Vec<Park>,
    /// The held lists the active and parked states reference.
    lists: Vec<(String, Held)>,
}

/// What a save does with a parked queue.
enum Park {
    /// It was adjusted: it is kept.
    Keep(QueueRow),
    /// It was not: it goes, with the lists only it used.
    Delete(String),
}

/// A queue ready to write: its state is serialized and the hashes it references are known.
struct QueueRow {
    queue_id: String,
    version: u32,
    context: String,
    state: String,
    /// The position as JSON; only a parked queue's is written.
    position: Option<String>,
    hashes: Vec<String>,
}

/// One account a write saved. Once the write commits, pass it to [`Queues::committed`].
pub(super) struct Saved {
    user: i64,
    /// The hashes of the held lists the commit stored.
    stored: HashSet<String>,
    /// The IDs of the parked queues the commit wrote.
    parked: Vec<String>,
}

/// What an account's save takes from memory, before it is serialized.
pub(super) struct Taken {
    active: Queue,
    parked: Vec<Queue>,
    lists: Vec<(String, Held)>,
}

impl Queues {
    /// Saves changed accounts every [`SAVE_EVERY`] until the returned task is aborted, which
    /// leaves a write already begun to finish. Call [`save`](Self::save) once more after.
    pub fn keep_saved(self: &Arc<Self>, db: Arc<Database>) -> JoinHandle<()> {
        let queues = self.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(SAVE_EVERY);
            // After a stall, such as a long write by the scanner, one save covers what was missed.
            every.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                queues.save(&db).await;
            }
        })
    }

    /// Saves every account changed since it was last saved, in one write, each in its own
    /// savepoint. An account whose savepoint fails stays changed, with its held lists, to be
    /// tried again; the others are saved.
    pub async fn save(self: &Arc<Self>, db: &Database) {
        let queues = self.clone();
        match db.write(move |tx| Ok(queues.write_changed(tx))).await {
            Ok(saved) => self.committed(saved),
            Err(error) => {
                tracing::warn!(%error, "cannot save queues");
                // Which accounts the write took is lost with it, so all are tried again.
                for account in self.lock().values_mut() {
                    account.changed = true;
                }
            }
        }
    }

    /// Writes every changed account in `tx`, and returns those it wrote. Runs on the writer.
    /// The queues are serialized here, not on an async thread, since a large one takes a while.
    pub(super) fn write_changed(&self, tx: &Transaction) -> Vec<Saved> {
        let users: Vec<i64> = self
            .lock()
            .iter()
            .filter(|(_, account)| account.changed)
            .map(|(&user, _)| user)
            .collect();
        let mut saved = Vec::new();
        for user in users {
            let Some(pending) = self.take(user) else { continue };
            match write_account(tx, user, &pending) {
                Ok(()) => saved.push(pending.saved(user)),
                Err(error) => {
                    tracing::warn!(%error, user, "cannot save an account's queue");
                    if let Some(account) = self.lock().get_mut(&user) {
                        account.changed = true;
                    }
                }
            }
        }
        saved
    }

    /// Takes `user`'s changed state, parked queues and the held lists they reference, and counts
    /// the account as taken. The lock is held only to copy, so serializing does not stall other
    /// accounts. `None` if the account is gone or a state cannot be serialized, which a state
    /// never fails.
    pub(super) fn take(&self, user: i64) -> Option<Pending> {
        let taken = self.lock().get_mut(&user)?.take();
        taken.serialize()
    }

    /// Settles memory after a commit that wrote `saved`, under the lock. Written parked queues
    /// leave memory. A held list stays only if the account's changed states, still to be
    /// written, reference it and the commit did not store it: one added during the save. The
    /// rest are stored or no longer used.
    pub(super) fn committed(&self, saved: Vec<Saved>) {
        let mut accounts = self.lock();
        for Saved { user, stored, parked } in saved {
            let Some(account) = accounts.get_mut(&user) else { continue };
            account.parked.retain(|queue| !parked.iter().any(|id| id == queue.queue_id.as_str()));
            let referenced: HashSet<String> = account
                .parked
                .iter()
                .map(|queue| &queue.state)
                .chain([&account.state])
                .flat_map(|state| state.list_hashes())
                .map(str::to_owned)
                .collect();
            let changed = account.changed;
            account
                .lists
                .retain(|hash, _| changed && referenced.contains(hash) && !stored.contains(hash));
        }
    }
}

impl Account {
    /// Copies what a save needs, and counts the account as taken.
    pub(super) fn take(&mut self) -> Taken {
        self.changed = false;
        let active = self.active();
        let parked = self.parked.clone();
        let mut seen = HashSet::new();
        // An untouched parked queue is only deleted, so its lists are not written first.
        let lists = parked
            .iter()
            .filter(|queue| queue.state.adjusted)
            .map(|queue| &queue.state)
            .chain([&active.state])
            .flat_map(|state| state.list_hashes())
            .filter(|hash| seen.insert(*hash))
            .filter_map(|hash| Some((hash.to_owned(), self.lists.get(hash)?.clone())))
            .collect();
        Taken { active, parked, lists }
    }
}

impl Taken {
    /// Serializes the queues. `None` if one cannot be, which a state never fails.
    pub(super) fn serialize(self) -> Option<Pending> {
        let parked = self
            .parked
            .iter()
            .map(|queue| {
                let row = row(queue)?;
                Some(if queue.state.adjusted {
                    Park::Keep(row)
                } else {
                    Park::Delete(row.queue_id)
                })
            })
            .collect::<Option<_>>()?;
        Some(Pending { queue: row(&self.active)?, parked, lists: self.lists })
    }
}

impl Pending {
    /// What the commit of this save stored and wrote.
    pub(super) fn saved(self, user: i64) -> Saved {
        Saved {
            user,
            stored: self.lists.into_iter().map(|(hash, _)| hash).collect(),
            parked: self.parked.iter().map(Park::queue_id).map(str::to_owned).collect(),
        }
    }
}

impl Park {
    /// The ID of the queue this step is for.
    fn queue_id(&self) -> &str {
        match self {
            Park::Keep(row) => &row.queue_id,
            Park::Delete(id) => id,
        }
    }
}

/// `queue` serialized for writing.
fn row(queue: &Queue) -> Option<QueueRow> {
    let context = serde_json::to_string::<Value>(&queue.context).ok()?;
    let state = serde_json::to_string(&queue.state).ok()?;
    let position = match &queue.position {
        Some(position) => Some(serde_json::to_string(position).ok()?),
        None => None,
    };
    let hashes = queue.state.list_hashes().into_iter().map(str::to_owned).collect();
    Some(QueueRow {
        queue_id: queue.queue_id.as_str().to_owned(),
        version: queue.version,
        context,
        state,
        position,
        hashes,
    })
}

/// Writes one account in a savepoint, undoing all of it if any statement fails.
pub(super) fn write_account(
    tx: &Transaction,
    user: i64,
    pending: &Pending,
) -> rusqlite::Result<()> {
    tx.execute_batch("SAVEPOINT queue_save")?;
    let written = write_statements(tx, user, pending);
    match written {
        Ok(()) => tx.execute_batch("RELEASE queue_save"),
        Err(error) => {
            // Rolled back to, then released, or the savepoint would stay open in the transaction.
            tx.execute_batch("ROLLBACK TO queue_save; RELEASE queue_save")?;
            Err(error)
        }
    }
}

/// The statements of one account's save, in the order that keeps every reference whole.
fn write_statements(tx: &Transaction, user: i64, pending: &Pending) -> rusqlite::Result<()> {
    // Parked queues go first, so the active row, which may be one of them until now, is
    // demoted before the new active row is written, and the one-active index holds throughout.
    for park in &pending.parked {
        match park {
            Park::Keep(queue) => write_parked(tx, user, queue)?,
            Park::Delete(id) => {
                tx.prepare_cached("DELETE FROM queues WHERE user_id = ?1 AND queue_id = ?2")?
                    .execute(params![user, id])?;
            }
        }
    }
    // A list dropped and re-added within the second still has its row, so these ignore a clash.
    let mut insert = tx.prepare_cached(
        "INSERT OR IGNORE INTO queue_lists (user_id, hash, library_id, tracks, length) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (hash, held) in &pending.lists {
        let (bytes, length) = (list_bytes(&held.tracks), held.tracks.len());
        insert.execute(params![user, hash, held.library_id, bytes, length])?;
    }
    write_queue(tx, user, &pending.queue)?;
    delete_unused_lists(tx, user)
}

/// Deletes the account's lists that no queue uses.
pub(super) fn delete_unused_lists(tx: &Transaction, user: i64) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "DELETE FROM queue_lists WHERE user_id = ?1 AND NOT EXISTS \
         (SELECT 1 FROM queue_list_uses u WHERE u.user_id = ?1 AND u.hash = queue_lists.hash)",
    )?
    .execute([user])?;
    Ok(())
}

/// Writes `queue` as a kept queue, parked now with its position, and rewrites which lists it
/// uses. An upsert, so the row of a queue that was active until now is updated in place.
fn write_parked(tx: &Transaction, user: i64, queue: &QueueRow) -> rusqlite::Result<()> {
    tx.prepare_cached(
        "INSERT INTO queues (user_id, queue_id, version, context, state, active, last_active, \
                             position) \
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7) \
         ON CONFLICT (user_id, queue_id) DO UPDATE SET \
             version = excluded.version, context = excluded.context, state = excluded.state, \
             active = 0, last_active = excluded.last_active, position = excluded.position",
    )?
    .execute(params![
        user,
        queue.queue_id,
        queue.version,
        queue.context,
        queue.state,
        crate::db::now_ms(),
        queue.position
    ])?;
    write_uses(tx, user, queue)
}

/// Writes `queue` as the account's active queue, an upsert so a clashing row is never quietly
/// deleted, parks any other active queue of the account, and rewrites which lists it uses.
fn write_queue(tx: &Transaction, user: i64, queue: &QueueRow) -> rusqlite::Result<()> {
    // Whatever else is active, such as a row the server could not read at startup, is parked
    // first, so it can never keep the one-active index from taking this queue.
    tx.prepare_cached(
        "UPDATE queues SET active = 0, last_active = ?3 \
         WHERE user_id = ?1 AND active = 1 AND queue_id <> ?2",
    )?
    .execute(params![user, queue.queue_id, crate::db::now_ms()])?;
    tx.prepare_cached(
        "INSERT INTO queues (user_id, queue_id, version, context, state, active) \
         VALUES (?1, ?2, ?3, ?4, ?5, 1) \
         ON CONFLICT (user_id, queue_id) DO UPDATE SET \
             version = excluded.version, context = excluded.context, \
             state = excluded.state, active = 1, last_active = NULL",
    )?
    .execute(params![user, queue.queue_id, queue.version, queue.context, queue.state])?;
    write_uses(tx, user, queue)
}

/// Rewrites which lists `queue` uses.
fn write_uses(tx: &Transaction, user: i64, queue: &QueueRow) -> rusqlite::Result<()> {
    tx.prepare_cached("DELETE FROM queue_list_uses WHERE user_id = ?1 AND queue_id = ?2")?
        .execute(params![user, queue.queue_id])?;
    let mut uses = tx.prepare_cached(
        "INSERT INTO queue_list_uses (user_id, queue_id, hash) VALUES (?1, ?2, ?3)",
    )?;
    for hash in &queue.hashes {
        uses.execute(params![user, queue.queue_id, hash])?;
    }
    Ok(())
}
