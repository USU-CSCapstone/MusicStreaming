//! Starting, restoring and deleting queues (`design/queue.md` §8).
//!
//! Each runs as one job on the database's writer, which takes what it needs from memory under
//! the lock, so none can meet a half-done save, expiry or one another. A start only changes
//! memory, and the background save writes it; a restore saves its account at once, and a
//! delete removes rows.

use std::sync::{Arc, Mutex};

use jewelcase_core::{QueueError, QueueState, Recipe, Recover, Source};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;

use super::hash::list_hash;
use super::save::{delete_unused_lists, write_account};
use super::store::{Account, Position, Queue, Queues};
use crate::api::Uuid;
use crate::db::{Database, DbError};

/// Why a start, restore or delete did nothing.
#[derive(Debug, thiserror::Error)]
pub enum KeepError {
    /// Start: a queue of the account, active, kept or parked, already has this ID. A retried
    /// start lands here, and nothing changed.
    #[error("the account already has a queue with that ID")]
    Exists,
    /// Start: the list has no row at the position it starts at, or is empty.
    #[error("the start is not a position of the list")]
    NotInList(QueueError),
    /// Restore or delete: there is no such kept queue. Expired, deleted, never kept, or a
    /// parked queue that was not adjusted, which is due for deletion.
    #[error("no such kept queue")]
    NotFound,
    /// Restore or delete: the queue is the active one, which is neither restored nor deleted.
    #[error("the queue is the active one")]
    Active,
    /// The database failed. Nothing changed: a restore's memory is as it was.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<rusqlite::Error> for KeepError {
    /// A database failure.
    fn from(error: rusqlite::Error) -> Self {
        DbError::from(error).into()
    }
}

/// A queue to start: one list to play from position `at`, with the Manual Queue empty.
pub struct NewQueue {
    /// Chosen by the device.
    pub queue_id: Uuid,
    /// What the queue starts from, as the device sent it.
    pub context: Value,
    /// The source's key, chosen by the device; the caller has checked it is a UUID.
    pub key: String,
    /// The library every track of the list belongs to.
    pub library_id: i64,
    /// The list's track IDs.
    pub tracks: Arc<[u64]>,
    /// How the list was made, when it was made from a recipe.
    pub recipe: Option<Recipe>,
    /// The position that plays first.
    pub at: u64,
}

/// What a write took out of memory, to put back if it fails: the queue it removed, and for a
/// restore the active queue it replaced. A failed save puts them back itself; the caller does it
/// if the commit fails.
type Undo = Arc<Mutex<Option<(Option<Queue>, Option<Queue>)>>>;

/// The account `new` starts, with its list held. Hashes the list.
pub(super) fn fresh(new: NewQueue) -> Result<Account, KeepError> {
    let tracks = new.tracks;
    let mut source =
        Source::new(new.key, new.library_id.to_string(), tracks.len() as u64, new.recipe);
    source.hash = Some(list_hash(&tracks));
    let mut state = QueueState::default();
    state.start(source, new.at).map_err(KeepError::NotInList)?;
    let mut fresh = Account::new(new.queue_id, new.context, state);
    fresh.hold(new.library_id, tracks);
    Ok(fresh)
}

impl Queues {
    /// Starts `new` as `user`'s active queue and parks the one it replaces: kept at the next
    /// save if it was adjusted, deleted otherwise. `place` is called under the lock for where
    /// playback is, and it is kept with the parked queue if it names one of its items.
    ///
    /// Changes nothing and answers [`KeepError::Exists`] if `new.queue_id` names any queue of the
    /// account, so a retried start never revives or replaces one. Nothing is written until the
    /// next background save.
    pub async fn start(
        self: &Arc<Self>,
        db: &Database,
        user: i64,
        new: NewQueue,
        place: impl FnOnce() -> Option<Position> + Send + 'static,
    ) -> Result<(), KeepError> {
        let fresh = fresh(new)?;
        let queues = self.clone();
        // On the writer so that a queue a save is writing as kept is not missed.
        db.try_write(move |tx| queues.start_in(tx, user, fresh, place)).await
    }

    /// Makes `fresh`'s queue active, in the writer's job `tx`.
    pub(super) fn start_in(
        &self,
        tx: &Transaction,
        user: i64,
        fresh: Account,
        place: impl FnOnce() -> Option<Position>,
    ) -> Result<(), KeepError> {
        let stored = tx
            .prepare_cached("SELECT 1 FROM queues WHERE user_id = ?1 AND queue_id = ?2")?
            .exists(params![user, fresh.queue_id.as_str()])?;
        let mut accounts = self.lock();
        let Some(account) = accounts.get_mut(&user) else {
            if stored {
                return Err(KeepError::Exists);
            }
            accounts.insert(user, fresh);
            return Ok(());
        };
        if stored || account.names(&fresh.queue_id) {
            return Err(KeepError::Exists);
        }
        let queue = Queue {
            queue_id: fresh.queue_id,
            version: 0,
            context: fresh.context,
            state: fresh.state,
            position: None,
        };
        account.lists.extend(fresh.lists);
        account.park_for(queue, place());
        Ok(())
    }

    /// Makes the kept queue `queue_id` of `user` active, parks the active queue as a start does,
    /// and saves the account, in one write. Returns where the queue's playback was left.
    ///
    /// A parked queue not yet saved is taken from memory if it was adjusted, and is not found
    /// if it was not. Otherwise it is read from the database, where an expired queue is not
    /// found. If the save fails, memory is as it was, so a queue the database holds as kept is
    /// never playing.
    pub async fn restore(
        self: &Arc<Self>,
        db: &Database,
        user: i64,
        queue_id: Uuid,
        place: impl FnOnce() -> Option<Position> + Send + 'static,
    ) -> Result<Option<Position>, KeepError> {
        let undo: Undo = Arc::default();
        let queues = self.clone();
        let slot = undo.clone();
        let restored =
            db.try_write(move |tx| queues.restore_in(tx, user, &queue_id, place, &slot)).await;
        match restored {
            Ok((position, saved)) => {
                self.committed(vec![saved]);
                Ok(position)
            }
            Err(error) => {
                if let Some((target, active)) = undo.lock().recover().take() {
                    self.put_back(user, target, active);
                }
                Err(error)
            }
        }
    }

    /// The restore's job on the writer. Records in `undo` what it must put back.
    fn restore_in(
        &self,
        tx: &Transaction,
        user: i64,
        queue_id: &Uuid,
        place: impl FnOnce() -> Option<Position>,
        undo: &Undo,
    ) -> Result<(Option<Position>, super::save::Saved), KeepError> {
        let parked = match self.lock().get(&user) {
            Some(account) if account.queue_id == *queue_id => return Err(KeepError::Active),
            Some(account) => account
                .parked
                .iter()
                .find(|queue| queue.queue_id == *queue_id)
                .map(|q| q.state.adjusted),
            None => None,
        };
        let stored = match parked {
            Some(true) => None,
            Some(false) => return Err(KeepError::NotFound),
            None => Some(read_kept(tx, user, queue_id)?.ok_or(KeepError::NotFound)?),
        };
        let (position, taken) = {
            let mut accounts = self.lock();
            let target = match stored {
                Some(queue) => queue,
                None => {
                    let account = accounts.get_mut(&user).ok_or(KeepError::NotFound)?;
                    let i = account.parked.iter().position(|q| q.queue_id == *queue_id);
                    account.parked.remove(i.ok_or(KeepError::NotFound)?)
                }
            };
            let from_memory = parked.is_some().then(|| target.clone());
            let position = target.position.clone();
            let active = match accounts.get_mut(&user) {
                Some(account) => {
                    let previous = account.active();
                    account.park_for(target, place());
                    Some(previous)
                }
                None => {
                    accounts.insert(user, Account::from_queue(target));
                    None
                }
            };
            *undo.lock().recover() = Some((from_memory, active));
            let account = accounts.get_mut(&user).ok_or(KeepError::NotFound)?;
            (position, account.take())
        };
        let pending = taken.serialize().ok_or(KeepError::NotFound)?;
        if let Err(error) = write_account(tx, user, &pending) {
            // Put back here, on the writer, so no start or restore can run in between.
            if let Some((target, active)) = undo.lock().recover().take() {
                self.put_back(user, target, active);
            }
            return Err(error.into());
        }
        Ok((position, pending.saved(user)))
    }

    /// Puts memory back as it was before a restore: `active` is the queue it replaced, and
    /// `target` the one it took, if it came from the parked ones. Edits made to the restored
    /// queue meanwhile are dropped with it.
    fn put_back(&self, user: i64, target: Option<Queue>, active: Option<Queue>) {
        let mut accounts = self.lock();
        match active {
            Some(active) => {
                let Some(account) = accounts.get_mut(&user) else { return };
                account.parked.retain(|queue| queue.queue_id != active.queue_id);
                account.swap(active);
                account.parked.extend(target);
                account.changed = true;
            }
            None => {
                accounts.remove(&user);
            }
        }
    }
}

impl Queues {
    /// Deletes the kept queue `queue_id` of `user` and the account's lists no queue uses, in one
    /// write.
    ///
    /// The active queue is refused, decided from memory, since a parked queue not yet saved is
    /// still active in the database. A parked queue that was not adjusted is not found, as it is
    /// due for deletion anyway. An adjusted one is dropped from memory, then its row and lists
    /// go; if the write fails it is put back.
    pub async fn delete(
        self: &Arc<Self>,
        db: &Database,
        user: i64,
        queue_id: Uuid,
    ) -> Result<(), KeepError> {
        let removed: Arc<Mutex<Option<Queue>>> = Arc::default();
        let queues = self.clone();
        let slot = removed.clone();
        let deleted = db.try_write(move |tx| queues.delete_in(tx, user, &queue_id, &slot)).await;
        if deleted.is_err()
            && let Some(queue) = removed.lock().recover().take()
            && let Some(account) = self.lock().get_mut(&user)
        {
            account.parked.push(queue);
        }
        deleted
    }

    /// The delete's job on the writer. Records in `removed` the parked queue it took.
    fn delete_in(
        &self,
        tx: &Transaction,
        user: i64,
        queue_id: &Uuid,
        removed: &Mutex<Option<Queue>>,
    ) -> Result<(), KeepError> {
        let parked = {
            let mut accounts = self.lock();
            match accounts.get_mut(&user) {
                Some(account) if account.queue_id == *queue_id => return Err(KeepError::Active),
                Some(account) => {
                    match account.parked.iter().position(|q| q.queue_id == *queue_id) {
                        Some(i) if !account.parked[i].state.adjusted => {
                            return Err(KeepError::NotFound);
                        }
                        Some(i) => {
                            // Marked changed so the next save lets go of lists only it referenced.
                            account.changed = true;
                            Some(account.parked.remove(i))
                        }
                        None => None,
                    }
                }
                None => None,
            }
        };
        let was_parked = parked.is_some();
        *removed.lock().recover() = parked;
        let deleted = tx
            .prepare_cached("DELETE FROM queues WHERE user_id = ?1 AND queue_id = ?2")?
            .execute(params![user, queue_id.as_str()])?;
        if deleted == 0 && !was_parked {
            return Err(KeepError::NotFound);
        }
        delete_unused_lists(tx, user)?;
        Ok(())
    }
}

/// The kept queue `queue_id` of `user`, if it is stored as one and can be read.
fn read_kept(tx: &Transaction, user: i64, queue_id: &Uuid) -> rusqlite::Result<Option<Queue>> {
    let row = tx
        .prepare_cached(
            "SELECT version, context, state, position FROM queues \
             WHERE user_id = ?1 AND queue_id = ?2 AND active = 0",
        )?
        .query_row(params![user, queue_id.as_str()], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .optional()?;
    let Some((version, context, state, position)) = row else { return Ok(None) };
    let (Ok(context), Ok(state)) = (serde_json::from_str(&context), serde_json::from_str(&state))
    else {
        tracing::warn!(user, "cannot read a kept queue");
        return Ok(None);
    };
    let position = position.and_then(|text| serde_json::from_str(&text).ok());
    Ok(Some(Queue { queue_id: queue_id.clone(), version, context, state, position }))
}
