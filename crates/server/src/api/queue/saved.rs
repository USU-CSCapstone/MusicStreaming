//! Queues kept across restarts (`requirements/queue.md` §8), in `queues` and `queue_sources`.
//!
//! A change only marks its account's queue as unsaved. One task saves every unsaved queue
//! together every [`SAVE_EVERY`], and once more as the server stops, so the database sees one
//! write however many people are listening. A source is written once, as it joins the queue, so
//! what a shuffle or a move saves is the queue's state alone.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use jewelcase_core::{Queue, Source};
use rusqlite::{Connection, params};
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::{Account, Queues};
use crate::api::Uuid;
use crate::db::Database;

/// How long a change may go unsaved: as much of a queue as a crash can lose anyone.
const SAVE_EVERY: Duration = Duration::from_secs(1);

/// One account's queue, serialized and ready to write.
struct Row {
    user: i64,
    queue_id: String,
    version: u32,
    context: String,
    state: String,
    /// Whether the sources the database holds for the account are another queue's, so go first.
    replace: bool,
    /// The sources not yet saved, with the position of the first.
    first: usize,
    sources: Vec<(String, String, String)>,
}

impl Queues {
    /// Every active account's saved queue, as the server starts. A queue that cannot be read
    /// back is left out and logged, so one damaged row does not keep the server from starting.
    pub async fn load(db: &Database) -> Result<Queues, crate::db::DbError> {
        let accounts = std::sync::Mutex::new(db.read(load).await?);
        Ok(Queues { accounts })
    }

    /// Saves unsaved queues every [`SAVE_EVERY`] until the returned task is aborted, which leaves
    /// a write already begun to finish. Call [`save`](Self::save) once more after.
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

    /// Saves every queue changed since it was last saved, in one write. The queues are
    /// serialized off the async threads, since a large one takes a while.
    pub async fn save(self: &Arc<Self>, db: &Database) {
        let queues = self.clone();
        let rows = tokio::task::spawn_blocking(move || queues.unsaved()).await;
        let Ok(rows) = rows else {
            tracing::warn!("cannot serialize queues to save");
            return;
        };
        if rows.is_empty() {
            return;
        }
        let users: Vec<i64> = rows.iter().map(|row| row.user).collect();
        let write = db.write(move |tx| {
            for row in rows {
                // One account's failing, as when it was deleted under its queue, must not keep
                // everyone else's from being saved.
                if let Err(error) = write(tx, &row) {
                    tracing::warn!(%error, user = row.user, "cannot save an account's queue");
                }
            }
            Ok(())
        });
        if let Err(error) = write.await {
            tracing::warn!(%error, "cannot save queues");
            // Tried again at the next save, whole, not left until each queue's next change.
            let mut accounts = self.lock();
            for user in users {
                if let Some(account) = accounts.get_mut(&user) {
                    account.unsaved = true;
                    account.saved = None;
                }
            }
        }
    }

    /// Serializes every unsaved queue, and counts it as saved.
    fn unsaved(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (&user, account) in self.lock().iter_mut().filter(|(_, account)| account.unsaved) {
            let (replace, first) = match account.saved {
                Some(count) => (false, count),
                None => (true, 0),
            };
            let sources = account.queue.sources();
            let (Ok(state), Ok(context)) = (
                serde_json::to_string(&account.queue.stored()),
                serde_json::to_string(&account.context),
            ) else {
                continue;
            };
            let new = sources[first..].iter().filter_map(|source| {
                let tracks = serde_json::to_string(&source.tracks).ok()?;
                Some((source.key.clone(), source.library_id.clone(), tracks))
            });
            rows.push(Row {
                user,
                queue_id: account.queue_id.as_str().to_owned(),
                version: account.version,
                context,
                state,
                replace,
                first,
                sources: new.collect(),
            });
            account.unsaved = false;
            account.saved = Some(sources.len());
        }
        rows
    }
}

/// Writes one account's `row`: its new sources, then the queue.
fn write(tx: &rusqlite::Transaction, row: &Row) -> rusqlite::Result<()> {
    if row.replace {
        tx.execute("DELETE FROM queue_sources WHERE user_id = ?1", [row.user])?;
    }
    let mut insert = tx.prepare_cached(
        "INSERT INTO queue_sources (user_id, position, key, library_id, tracks) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (at, (key, library, tracks)) in row.sources.iter().enumerate() {
        insert.execute(params![row.user, row.first + at, key, library, tracks])?;
    }
    tx.prepare_cached(
        "INSERT OR REPLACE INTO queues (user_id, queue_id, version, context, state) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![row.user, row.queue_id, row.version, row.context, row.state])?;
    Ok(())
}

/// Each active account's saved queue, by user ID.
fn load(conn: &Connection) -> rusqlite::Result<HashMap<i64, Account>> {
    let mut select = conn.prepare(
        "SELECT q.user_id, q.queue_id, q.version, q.context, q.state \
         FROM queues q JOIN users u ON u.id = q.user_id WHERE u.status = 'active'",
    )?;
    let queues = select
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?
        .collect::<rusqlite::Result<Vec<(i64, String, u32, String, String)>>>()?;
    let mut sources = conn.prepare(
        "SELECT key, library_id, tracks FROM queue_sources WHERE user_id = ?1 ORDER BY position",
    )?;
    let mut accounts = HashMap::new();
    for (user, queue_id, version, context, state) in queues {
        let held = sources
            .query_map([user], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<(String, String, String)>>>()?;
        match restore(queue_id, version, &context, &state, held) {
            Some(account) => {
                accounts.insert(user, account);
            }
            None => tracing::warn!(user, "cannot read an account's saved queue"),
        }
    }
    Ok(accounts)
}

/// A saved queue's account, or `None` if what was saved does not read back.
fn restore(
    queue_id: String,
    version: u32,
    context: &str,
    state: &str,
    held: Vec<(String, String, String)>,
) -> Option<Account> {
    let sources = held
        .into_iter()
        .map(|(key, library_id, tracks)| {
            Some(Source { key, library_id, tracks: serde_json::from_str(&tracks).ok()? })
        })
        .collect::<Option<Vec<Source>>>()?;
    let count = sources.len();
    let mut queue: Queue = serde_json::from_str(state).ok()?;
    queue.attach_sources(sources);
    let queue_id = Uuid::try_from(queue_id).ok()?;
    Some(Account {
        saved: Some(count),
        queue_id,
        version,
        context: serde_json::from_str::<Value>(context).ok()?,
        queue,
        unsaved: false,
    })
}
