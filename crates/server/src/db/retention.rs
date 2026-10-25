//! The hourly sweep, which deletes what is kept only for a while (`design/database.md` §9).
//!
//! A task runs it every hour, first a few minutes after the server starts.
//! Rows go once they are older than [`WINDOW_MS`]: finished scans, but each library's last
//! finished and last completed; login attempts; ended plays' events for the `played` hook;
//! recent searches and what the `searched` hook noted of them; and tracks missing that long,
//! with whatever of the catalog goes with them. A request to forget searches (`stopped`) is
//! kept until it is delivered instead, however old it is. Art and biographies that nothing
//! points at go whatever their age.
//!
//! The sweep is one write per chunk of rows ([`sweep`]), so a write queued behind it waits for
//! one chunk, never the whole sweep. A kind of row that fails is passed over until the next
//! sweep.

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tracing::warn;

use jewelcase_plugins::Permission;
use rusqlite::{Result, Transaction, params};

use super::{Database, Text, now_ms};

mod catalog;
#[cfg(test)]
mod tests;

/// How long a row is kept: 30 days.
pub const WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// How long after the server starts the first sweep runs, so a server restarted more often than
/// [`EVERY`] still sweeps, without adding to its startup.
pub const FIRST_SWEEP: Duration = Duration::from_secs(300);

/// How often the sweep runs.
const EVERY: Duration = Duration::from_secs(3600);

/// The most rows one statement of the sweep deletes or looks at, so a write stays short.
/// Tracks go in smaller chunks ([`catalog::tracks`]).
const CHUNK: usize = 100;

/// One kind of row to sweep. Given the sweep's time and where it is in its table, it deletes one
/// chunk of rows and returns whether the kind is done.
type Stage = fn(&Transaction<'_>, i64, &mut i64) -> Result<bool>;

/// Every kind of row, in order: missing tracks before the art and biographies they leave
/// unreferenced.
const STAGES: [Stage; 9] = [
    scans,
    login_attempts,
    play_ends,
    recent_searches,
    searches_noted,
    forgets_delivered,
    catalog::tracks,
    catalog::images,
    catalog::biographies,
];

/// Sweeps every kind of row at `now`, one write per chunk, until none has more to delete. A
/// kind whose write fails, or panics, is logged and passed over until the next sweep, so one row
/// that cannot go never holds up the others.
pub async fn sweep(db: &Database, now: i64) {
    for stage in STAGES {
        let mut after = i64::MIN;
        loop {
            let chunk = db
                .write(move |tx| {
                    let mut at = after;
                    stage(tx, now, &mut at).map(|done| (done, at))
                })
                .await;
            match chunk {
                Ok((true, _)) => break,
                Ok((false, at)) => after = at,
                Err(error) => {
                    warn!(%error, "cannot sweep part of the database");
                    break;
                }
            }
        }
    }
}

/// Starts the task that sweeps `first` after it starts and every hour after, and returns its
/// handle. Aborting it stops the sweep before its next chunk.
pub fn start(db: Arc<Database>, first: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(first).await;
        loop {
            sweep(&db, now_ms()).await;
            tokio::time::sleep(EVERY).await;
        }
    })
}

/// Every library's ID.
fn libraries(tx: &Transaction<'_>) -> Result<Vec<i64>> {
    tx.prepare_cached("SELECT id FROM libraries")?.query_map([], |r| r.get(0))?.collect()
}

/// Runs `sql`, which deletes up to `?2` rows older than `?1`, with the sweep's cutoff and
/// [`CHUNK`], and returns whether it deleted fewer, so none is left.
fn aged(tx: &Transaction<'_>, now: i64, sql: &str) -> Result<bool> {
    Ok(tx.prepare_cached(sql)?.execute(params![now - WINDOW_MS, CHUNK])? < CHUNK)
}

/// Finished scans aged out, but in each library its last finished scan, which the admin sees,
/// and its last completed one, which says the library has been scanned, so its next startup
/// scan is not taken for its first.
fn scans(tx: &Transaction<'_>, now: i64, _: &mut i64) -> Result<bool> {
    let mut stmt = tx.prepare_cached(
        "DELETE FROM scans WHERE id IN (SELECT id FROM scans \
         WHERE library_id = ?3 AND created_at < ?1 AND finished_at < ?1 \
         AND id NOT IN (SELECT id FROM scans WHERE library_id = ?3 AND finished_at IS NOT NULL \
                        ORDER BY created_at DESC, id DESC LIMIT 1) \
         AND id NOT IN (SELECT id FROM scans WHERE library_id = ?3 AND state = 'completed' \
                        ORDER BY finished_at DESC LIMIT 1) \
         LIMIT ?2)",
    )?;
    let mut deleted = 0;
    for library in libraries(tx)? {
        deleted += stmt.execute(params![now - WINDOW_MS, CHUNK - deleted, library])?;
        if deleted == CHUNK {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Sign-in attempts aged out.
fn login_attempts(tx: &Transaction<'_>, now: i64, _: &mut i64) -> Result<bool> {
    aged(
        tx,
        now,
        "DELETE FROM login_attempts WHERE id IN \
         (SELECT id FROM login_attempts WHERE attempted_at < ?1 LIMIT ?2)",
    )
}

/// Ends of plays aged out of the `played` hook's outbox, delivered or not. Its starts go as
/// each new play is recorded (`0005_activity.sql`).
fn play_ends(tx: &Transaction<'_>, now: i64, _: &mut i64) -> Result<bool> {
    aged(
        tx,
        now,
        "DELETE FROM play_events WHERE seq IN \
         (SELECT seq FROM play_events WHERE kind = 'ended' AND reported_at < ?1 LIMIT ?2)",
    )
}

/// Recent searches aged out of the window (`requirements/search.md` §6).
fn recent_searches(tx: &Transaction<'_>, now: i64, _: &mut i64) -> Result<bool> {
    aged(
        tx,
        now,
        "DELETE FROM recent_searches WHERE id IN \
         (SELECT id FROM recent_searches WHERE searched_at < ?1 LIMIT ?2)",
    )
}

/// What the `searched` hook noted of searches and their removal, aged out with the searches.
fn searches_noted(tx: &Transaction<'_>, now: i64, _: &mut i64) -> Result<bool> {
    aged(
        tx,
        now,
        "DELETE FROM search_events WHERE seq IN (SELECT seq FROM search_events \
         WHERE kind <> 'stopped' AND created_at < ?1 LIMIT ?2)",
    )
}

/// Requests to forget searches that their plugin's `searched` hook has delivered in their
/// library, however recent. One not yet delivered is kept, however old.
fn forgets_delivered(tx: &Transaction<'_>, _: i64, _: &mut i64) -> Result<bool> {
    let deleted = tx
        .prepare_cached(
            "DELETE FROM search_events WHERE seq IN (SELECT e.seq FROM search_events e \
         WHERE e.kind = 'stopped' AND EXISTS (SELECT 1 FROM plugin_cursors c \
             WHERE c.plugin_id = e.plugin_id AND c.library_id = e.library_id AND c.hook = ?1 \
             AND c.user_id = e.user_id AND c.position >= e.seq) \
         LIMIT ?2)",
        )?
        .execute(params![Text(Permission::Searched), CHUNK])?;
    Ok(deleted < CHUNK)
}
