//! The hourly sweep, which deletes what is kept only for a while (`design/database.md` §9).
//!
//! The writer runs it every hour, first a few minutes after the database opens.
//! Rows go once they are older than [`WINDOW_MS`]: finished scans, but each library's last
//! finished and last completed; login attempts; ended plays' events for the `played` hook;
//! recent searches and what the `searched` hook noted of them; and tracks missing that long,
//! with whatever of the catalog goes with them. A request to forget searches (`stopped`) is
//! kept until it is delivered instead, however old it is. Art and biographies that nothing
//! points at go whatever their age.
//!
//! The sweep runs as a chain of transactions ([`Sweep::step`]), each holding the writer for
//! about [`STEP_TIME`], so a write queued behind it waits for one step, never the whole sweep.
//! A step sweeps one kind of row, so a kind that fails can be passed over ([`Sweep::next`]).

use std::time::{Duration, Instant};

use jewelcase_plugins::Permission;
use rusqlite::{Result, Transaction, params};

use super::Text;

mod catalog;
#[cfg(test)]
mod tests;

/// How long a row is kept: 30 days.
pub const WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// About how long one step of the sweep holds the writer: as long as a scan batch does
/// (`design/scanning.md` §7), so an interactive write waits no longer behind it.
pub const STEP_TIME: Duration = Duration::from_millis(50);

/// The most rows one statement of the sweep deletes or looks at, so a step can stop on time.
/// Tracks go in smaller chunks ([`catalog::tracks`]).
const CHUNK: usize = 100;

/// One kind of row to sweep. Given the sweep's time and where it is in its table, it deletes a
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

/// A sweep under way.
#[derive(Clone, Copy)]
pub struct Sweep {
    /// When it started: rows older than [`WINDOW_MS`] before this go.
    now: i64,
    /// About how long each step runs.
    step_time: Duration,
    /// The index in [`STAGES`] of the kind it is sweeping.
    stage: usize,
    /// Where that kind is in its table, for those that walk it by ID.
    after: i64,
}

impl Sweep {
    /// A sweep at `now`, in steps of about `step_time`. A zero step time sweeps one chunk of
    /// one kind of row per step.
    pub fn new(now: i64, step_time: Duration) -> Sweep {
        Sweep { now, step_time, stage: 0, after: i64::MIN }
    }

    /// Sweeps one kind of row in `tx`, for about the step time or until the kind is done, and
    /// returns the rest of the sweep, if any.
    pub fn step(mut self, tx: &Transaction<'_>) -> Result<Option<Sweep>> {
        let started = Instant::now();
        let Some(stage) = STAGES.get(self.stage) else { return Ok(None) };
        loop {
            if stage(tx, self.now, &mut self.after)? {
                return Ok(self.next());
            }
            if started.elapsed() >= self.step_time {
                return Ok(Some(self));
            }
        }
    }

    /// The rest of the sweep after the kind of row it is sweeping, if any: where a step leaves
    /// it once the kind is done, and where its writer moves it past a step that failed.
    pub fn next(mut self) -> Option<Sweep> {
        self.stage += 1;
        self.after = i64::MIN;
        (self.stage < STAGES.len()).then_some(self)
    }
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
