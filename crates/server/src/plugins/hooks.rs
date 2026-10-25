//! Hooks: running plugins when something happens, rather than when an admin asks
//! (`requirements/plugins.md` §8).
//!
//! `tracksChanged`, `albumsChanged`, and `artistsChanged` read the library change feed
//! (`0004_feeds.sql`), which records every track, album, and artist added, changed, or removed
//! in the same transaction as the change, each hook only its own kind; `scanFinished`
//! reads finished scans; `schedule` runs at the interval the plugin asks for; `played` reads
//! the plays each user who connected the plugin has ended, and `playing` those they have just
//! started (`0005_activity.sql`). For each hook, each plugin and library pair keeps its own
//! position (`plugin_cursors`), and in `played` and `playing` each user too, which moves only
//! once the plugin has handled what came before it.
//! `playing` alone never catches up: what was playing a while ago is no longer news. `searched`
//! reads the searches of users who share them with the plugin, and those they ask it to forget
//! (`0005_activity.sql`), with a position per user as well.
//! A failure is retried later from the same place, so nothing is lost to a crash, a restart,
//! or a service that was briefly down. One that keeps failing is disabled in that library, and
//! the admin is told why (§11), except in the hooks that act for one user (`played`, `playing`,
//! `searched`): one user's failing account must not stop the plugin for everyone, so theirs
//! keeps being retried, at most an hour apart, instead.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use jewelcase_plugins::{Event, Permission};
use rusqlite::{Connection, OptionalExtension, params};
use tokio::sync::Notify;

mod changes;
mod delivering;
mod due;
mod playing;
mod plays;
mod searches;

use super::run::RunFor;
use super::{Plugins, grants};
use crate::db::{Text, now_ms};
use delivering::Delivering;

/// How often the dispatcher looks for hooks that are due.
const POLL: Duration = Duration::from_secs(5);
/// Changes delivered in one event. Small enough that a plugin making a request per track
/// finishes a batch well within a run's time limit.
const BATCH: i64 = 100;
/// How long a plugin may fail, with no success between, before it is disabled in a library.
const MAX_FAILING_MS: i64 = 72 * 60 * 60_000;
/// The shortest wait after a failure. The wait is as long as the plugin has been failing, up to
/// [`MAX_BACKOFF_MS`].
const BACKOFF_MS: i64 = 60_000;
/// The longest wait between retries.
const MAX_BACKOFF_MS: i64 = 60 * 60_000;

/// A hook due for a plugin in a library, and how far delivery has got.
#[derive(Clone)]
struct Due {
    hook: Permission,
    /// How its event is read.
    read: due::Read,
    plugin: String,
    library: i64,
    position: i64,
    /// The user it is due for, in `played`, `playing`, and `searched`, and acts for except in a
    /// request to forget ([`Due::run_for`]). Its cursor stores none as 0.
    user: Option<i64>,
}

impl Due {
    /// Where a run of `event` is, and who it acts for: the library, and the user, except for a
    /// request to forget searches, which names its user itself and runs with the shared settings
    /// alone, so it reaches the plugin even once they can no longer be acted for there.
    fn run_for(&self, event: &Event) -> RunFor {
        let forget = matches!(event, Event::SearchesForgotten(_));
        RunFor { library: self.library, user: self.user.filter(|_| !forget) }
    }
}

/// How a delivery went, for its position and retries.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Ran {
    /// Handled, or nothing to hand the plugin, so the position moves on.
    Handled,
    /// Failed, so it is tried again later. It disables the plugin once failures have gone on
    /// for [`MAX_FAILING_MS`].
    Failed,
    /// Failed on the server's side, such as for a full disk. It is tried again later, and counts
    /// toward nothing.
    Unrun,
}

impl Plugins {
    /// Starts delivering hook events in the background, until the plugins are dropped: every
    /// [`POLL`] while a plugin is enabled in any library, and otherwise not until an admin
    /// enables one. The loop runs on the Tokio runtime it is called on, so this panics outside
    /// one, and only finds what is due; the deliveries run on the plugin runtime, which the first
    /// one starts.
    pub fn start_hooks(self: &Arc<Plugins>) {
        let (plugins, wake) = (Arc::downgrade(self), self.wake_hooks.clone());
        tokio::spawn(async move {
            while let Some(plugins) = plugins.upgrade() {
                let enabled = plugins.dispatch().await;
                drop(plugins);
                idle(enabled, &wake).await;
            }
        });
    }

    /// Stops starting hook deliveries, as the server stops. Those already running go on; those
    /// still waiting for their turn or a slot run nothing.
    pub fn stop_hooks(&self) {
        self.hooks_stopped.store(true, Ordering::Relaxed);
    }

    /// Starts each delivery that is due and not already under way. One plugin, library, hook,
    /// and user has at most one delivery at a time, and each waits for the plugin's turn in the
    /// library ([`Plugins::deliver`]). Answers false if no plugin is enabled in any library, so
    /// nothing can be due until one is, and, starting nothing, once the hooks are stopped
    /// ([`Plugins::stop_hooks`]).
    pub async fn dispatch(self: &Arc<Plugins>) -> bool {
        if self.hooks_stopped.load(Ordering::Relaxed) {
            return false;
        }
        let read = self.db.read(|conn| {
            let enabled = due::any_enabled(conn)?;
            enabled.then(|| due::all(conn, now_ms())).transpose()
        });
        let due = match read.await {
            Ok(Some(due)) => due,
            Ok(None) => return false,
            Err(error) => {
                // A failed read is no sign that nothing is enabled, so polling goes on.
                tracing::error!(%error, "cannot find hook events to deliver");
                return true;
            }
        };
        for due in due {
            let Ok(runtime) = self.runtime() else { return true };
            let key = (due.plugin.clone(), due.library, due.hook, due.user);
            let Some(delivering) = Delivering::claim(self, key) else { continue };
            runtime.spawn(delivering.deliver(due));
        }
        true
    }

    /// Delivers what is due to one plugin in one library, and records how it went, in the
    /// plugin's turn there and once one of the [`Plugins::hook_slots`] is free. The turn comes
    /// first, so a delivery waiting for it holds no slot another plugin or library could use; it
    /// holds its turn while it waits for a slot. One no longer due once it has both, such as
    /// for a plugin disabled meanwhile or the hooks stopped, runs nothing and records nothing. One
    /// whose outcome cannot be recorded lets its turn and slot go, then waits [`BACKOFF_MS`]
    /// still holding its claim, so the same delivery, whose side effects may have happened, is
    /// not run again at the next poll.
    async fn deliver(&self, due: Due) {
        let turn = self.turns.take(&due.plugin, due.library).await;
        let slot = self.hook_slots.acquire().await.expect("the hook slots are never closed");
        if self.hooks_stopped.load(Ordering::Relaxed) {
            return;
        }
        let at = due.clone();
        let read = self
            .db
            .read(move |conn| {
                let now = now_ms();
                if !due::still(conn, &at, now)? {
                    return Ok(None);
                }
                Ok(Some((
                    (at.read)(conn, &at, now)?,
                    grants::manifest(conn, &at.plugin)?,
                    grants::granted(conn, &at.plugin, at.library)?,
                )))
            })
            .await;
        let ((next, event), manifest, permissions) = match read {
            Ok(Some((event, Some(manifest), permissions))) => (event, manifest, permissions),
            Ok(_) => return,
            Err(error) => return tracing::error!(%error, "cannot read hook events"),
        };
        // Nothing for the plugin, such as a start that went stale, moves the position on.
        let (ran, summary) = match event {
            None => (Ran::Handled, String::new()),
            Some(event) => {
                let run_for = due.run_for(&event);
                match self.run_in(&due.plugin, &manifest, run_for, permissions, event).await {
                    Ok((outcome, _)) => {
                        let ran = if outcome.result.is_ok() { Ran::Handled } else { Ran::Failed };
                        (ran, outcome.summary())
                    }
                    // `run_in` has logged it; an unrun delivery's summary is never shown.
                    Err(_) => (Ran::Unrun, String::new()),
                }
            }
        };
        let recorded =
            self.db.write(move |tx| record(tx, &due, next, ran, &summary, now_ms())).await;
        if let Err(error) = recorded {
            tracing::error!(%error, "cannot record a hook delivery");
            drop(slot);
            drop(turn);
            tokio::time::sleep(Duration::from_millis(BACKOFF_MS.unsigned_abs())).await;
        }
    }
}

/// Waits until the hook loop should dispatch again: [`POLL`] after a dispatch that found a plugin
/// `enabled`, and otherwise until `wake` is notified, as when an admin enables one.
async fn idle(enabled: bool, wake: &Notify) {
    if enabled {
        tokio::time::sleep(POLL).await;
    } else {
        wake.notified().await;
    }
}

/// Moves the position on to `next` once the delivery is handled, which also clears the plugin's
/// failing. After a failure it keeps the position and waits as long as the plugin has been
/// failing before trying again, at least [`BACKOFF_MS`] and at most [`MAX_BACKOFF_MS`]; after
/// one on the server's side ([`Ran::Unrun`]) it waits the same but does not start or extend the
/// failing. The failing is read from the stored row, so a delivery under way as the plugin is
/// enabled again counts from its fresh start. Unless it is due for one user, it disables the
/// plugin in the library once it has been failing for [`MAX_FAILING_MS`].
fn record(
    conn: &Connection,
    due: &Due,
    next: i64,
    ran: Ran,
    summary: &str,
    now: i64,
) -> rusqlite::Result<()> {
    let (plugin, library, hook) = (&due.plugin, due.library, Text(due.hook));
    let user = due.user.unwrap_or(0);
    let since: Option<i64> = conn
        .prepare_cached(
            "SELECT failing_since FROM plugin_cursors \
             WHERE plugin_id = ?1 AND library_id = ?2 AND hook = ?3 AND user_id = ?4",
        )?
        .query_row(params![plugin, library, hook, user], |row| row.get(0))
        .optional()?
        .flatten();
    let failing_since = match ran {
        Ran::Handled => {
            conn.execute(
                "INSERT INTO plugin_cursors (plugin_id, library_id, hook, user_id, position) \
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (plugin_id, library_id, hook, user_id) \
                 DO UPDATE SET position = excluded.position, failing_since = NULL, \
                 retry_at = NULL",
                params![plugin, library, hook, user, next],
            )?;
            return Ok(());
        }
        Ran::Failed => Some(since.unwrap_or(now)),
        Ran::Unrun => since,
    };
    let failing_for = now - failing_since.unwrap_or(now);
    let backoff = failing_for.clamp(BACKOFF_MS, MAX_BACKOFF_MS);
    conn.execute(
        "INSERT INTO plugin_cursors (plugin_id, library_id, hook, user_id, failing_since, retry_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT (plugin_id, library_id, hook, user_id) \
         DO UPDATE SET failing_since = excluded.failing_since, retry_at = excluded.retry_at",
        params![plugin, library, hook, user, failing_since, now + backoff],
    )?;
    if ran != Ran::Failed || failing_for < MAX_FAILING_MS {
        return Ok(());
    }
    let hours = MAX_FAILING_MS / (60 * 60_000);
    let stopped = format!("Stopped after {hours} hours of failing: {summary}");
    if due.user.is_none() {
        conn.execute(
            "UPDATE plugin_libraries SET enabled = 0, disabled_reason = ?3 \
             WHERE plugin_id = ?1 AND library_id = ?2",
            params![plugin, library, stopped],
        )?;
        tracing::warn!(
            plugin,
            library,
            hook = due.hook.name(),
            reason = stopped,
            "disabled a plugin"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
