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
//! the admin is told why (§11), except in `played`: one user's failing account must not stop
//! the plugin for everyone, so theirs is retried hourly instead.

use std::sync::Arc;
use std::time::Duration;

use jewelcase_plugins::Permission;
use rusqlite::{Connection, params};
use tokio::sync::Notify;

mod changes;
mod due;
mod playing;
mod plays;
mod searches;

use super::run::RunFor;
use super::{Plugins, grants};
use crate::db::{Text, now_ms};

/// How often the dispatcher looks for hooks that are due.
const POLL: Duration = Duration::from_secs(5);
/// Changes delivered in one event. Small enough that a plugin making a request per track
/// finishes a batch well within a run's time limit.
const BATCH: i64 = 100;
/// Failures in a row before a plugin is disabled in a library.
const MAX_FAILURES: i64 = 5;
/// How long a plugin may fail to reach its service, with no other failure between, before it
/// is disabled in a library.
const MAX_OUTAGE_MS: i64 = 72 * 60 * 60_000;
/// The wait after the first failure, doubling after each until [`MAX_BACKOFF_MS`].
const BACKOFF_MS: i64 = 60_000;
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
    failures: i64,
    outages: i64,
    /// When the outages since the last other failure or success began, if there are any.
    outage_since: Option<i64>,
    /// The user it acts for, in `played`, `playing`, and `searched`; otherwise 0.
    user: i64,
}

/// How a delivery went, for its position and retries.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Ran {
    /// Handled, or nothing to hand the plugin, so the position moves on.
    Handled,
    /// Failed, so it is tried again later, and counts toward disabling the plugin.
    Failed,
    /// Failed after the plugin found a service out of reach (`Outcome::outage`), so it is tried
    /// again later. It disables the plugin only once the outage has lasted [`MAX_OUTAGE_MS`].
    Unreachable,
}

impl Ran {
    /// How a run went, given whether it succeeded and whether it found a service out of reach
    /// on the way: one that succeeded is handled, even after a request that had to wait.
    fn of(succeeded: bool, outage: bool) -> Ran {
        match (succeeded, outage) {
            (true, _) => Ran::Handled,
            (false, true) => Ran::Unreachable,
            (false, false) => Ran::Failed,
        }
    }
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

    /// Starts each delivery that is due and not already under way. One plugin, library, hook,
    /// and user has at most one delivery at a time; different ones run side by side. Answers
    /// false if no plugin is enabled in any library, so nothing can be due until one is.
    pub async fn dispatch(self: &Arc<Plugins>) -> bool {
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
            if !self.delivering.lock().expect("delivery lock").insert(key.clone()) {
                continue;
            }
            let plugins = self.clone();
            runtime.spawn(async move {
                plugins.deliver(due).await;
                plugins.delivering.lock().expect("delivery lock").remove(&key);
            });
        }
        true
    }

    /// Delivers what is due to one plugin in one library, and records how it went.
    async fn deliver(&self, due: Due) {
        let at = due.clone();
        let read = self
            .db
            .read(move |conn| {
                Ok((
                    (at.read)(conn, &at, now_ms())?,
                    grants::manifest(conn, &at.plugin)?,
                    grants::granted(conn, &at.plugin, at.library)?,
                ))
            })
            .await;
        let ((next, event), manifest, permissions) = match read {
            Ok((event, Some(manifest), permissions)) => (event, manifest, permissions),
            Ok((_, None, _)) => return,
            Err(error) => return tracing::error!(%error, "cannot read hook events"),
        };
        // Nothing for the plugin, such as a start that went stale, moves the position on.
        let (ran, summary) = match event {
            None => (Ran::Handled, String::new()),
            Some(event) => {
                let run_for =
                    RunFor { library: due.library, user: (due.user != 0).then_some(due.user) };
                match self.run_in(&due.plugin, &manifest, run_for, permissions, event).await {
                    Ok((outcome, _)) => {
                        (Ran::of(outcome.result.is_ok(), outcome.outage), outcome.summary())
                    }
                    Err(error) => (Ran::Failed, error.to_string()),
                }
            }
        };
        let recorded =
            self.db.write(move |tx| record(tx, &due, next, ran, &summary, now_ms())).await;
        if let Err(error) = recorded {
            tracing::error!(%error, "cannot record a hook delivery");
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

/// Moves the position on to `next` once the delivery is handled. After a failure it keeps the
/// position and waits longer each time before trying again. Unless it is acting for one user, it
/// disables the plugin in the library once it has failed [`MAX_FAILURES`] times in a row, not
/// counting outages ([`Ran::Unreachable`]), or once it has failed to reach its service for
/// [`MAX_OUTAGE_MS`] with no other failure between: a failure of its own shows the service was
/// reached, or that the plugin is at fault, so it ends the outage.
fn record(
    conn: &Connection,
    due: &Due,
    next: i64,
    ran: Ran,
    summary: &str,
    now: i64,
) -> rusqlite::Result<()> {
    let (plugin, library, hook, user) = (&due.plugin, due.library, Text(due.hook), due.user);
    let (failures, outages, outage_since) = match ran {
        Ran::Handled => {
            conn.execute(
                "INSERT INTO plugin_cursors (plugin_id, library_id, hook, user_id, position) \
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (plugin_id, library_id, hook, user_id) \
                 DO UPDATE SET position = excluded.position, failures = 0, outages = 0, \
                 outage_since = NULL, retry_at = NULL",
                params![plugin, library, hook, user, next],
            )?;
            return Ok(());
        }
        Ran::Failed => (due.failures + 1, due.outages, None),
        Ran::Unreachable => (due.failures, due.outages + 1, Some(due.outage_since.unwrap_or(now))),
    };
    let tries = failures + outages;
    let backoff = (BACKOFF_MS << (tries - 1).min(10)).min(MAX_BACKOFF_MS);
    conn.execute(
        "INSERT INTO plugin_cursors \
             (plugin_id, library_id, hook, user_id, failures, outages, outage_since, retry_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT (plugin_id, library_id, hook, user_id) \
         DO UPDATE SET failures = excluded.failures, outages = excluded.outages, \
         outage_since = excluded.outage_since, retry_at = excluded.retry_at",
        params![plugin, library, hook, user, failures, outages, outage_since, now + backoff],
    )?;
    let stopped = match (ran, outage_since) {
        (Ran::Failed, _) if failures >= MAX_FAILURES => {
            format!("Stopped after failing {failures} times in a row: {summary}")
        }
        (Ran::Unreachable, Some(since)) if now - since >= MAX_OUTAGE_MS => {
            let hours = MAX_OUTAGE_MS / (60 * 60_000);
            format!("Stopped after it could not reach its service for {hours} hours: {summary}")
        }
        _ => return Ok(()),
    };
    if user == 0 {
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
