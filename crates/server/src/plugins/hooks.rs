//! Hooks: running plugins when something happens, rather than when an admin asks
//! (`requirements/plugins.md` §8).
//!
//! `tracksChanged` reads the library change feed (`0004_feeds.sql`), which records every
//! track added, changed, or removed in the same transaction as the change; `scanFinished`
//! reads finished scans; `schedule` runs at the interval the plugin asks for; `played` reads
//! the plays each user who connected the plugin has ended (`0013_plugin_played.sql`). For each
//! hook, each plugin and library pair keeps its own position (`plugin_cursors`), and in
//! `played` each user too, which moves only once the plugin has handled what came before it.
//! A failure is retried later from the same place, so nothing is lost to a crash, a restart,
//! or a service that was briefly down. One that keeps failing is disabled in that library, and
//! the admin is told why (§11), except in `played`: one user's failing account must not stop
//! the plugin for everyone, so theirs is retried hourly instead.

use std::sync::{Arc, Weak};
use std::time::Duration;

use rusqlite::{Connection, params};

mod due;
mod plays;

use super::{Plugins, grants};
use crate::db::now_ms;

/// How often the dispatcher looks for hooks that are due.
const POLL: Duration = Duration::from_secs(5);
/// Changes delivered in one event. Small enough that a plugin making a request per track
/// finishes a batch well within a run's time limit.
const BATCH: i64 = 100;
/// Failures in a row before a plugin is disabled in a library.
const MAX_FAILURES: i64 = 5;
/// The wait after the first failure, doubling after each until [`MAX_BACKOFF_MS`].
const BACKOFF_MS: i64 = 60_000;
const MAX_BACKOFF_MS: i64 = 60 * 60_000;

/// A hook, as the grants and cursors name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Hook {
    TracksChanged,
    ScanFinished,
    Schedule,
    Played,
}

impl Hook {
    fn name(self) -> &'static str {
        match self {
            Hook::TracksChanged => "tracksChanged",
            Hook::ScanFinished => "scanFinished",
            Hook::Schedule => "schedule",
            Hook::Played => "played",
        }
    }
}

/// A hook due for a plugin in a library, and how far delivery has got.
struct Due {
    hook: Hook,
    plugin: String,
    library: i64,
    position: i64,
    failures: i64,
    /// The user it acts for, in `played`; otherwise 0.
    user: i64,
}

impl Plugins {
    /// Starts delivering hook events in the background, until the plugins are dropped.
    pub fn start_hooks(self: &Arc<Plugins>) {
        let Ok(runtime) = self.runtime() else { return };
        let plugins: Weak<Plugins> = Arc::downgrade(self);
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(POLL).await;
                let Some(plugins) = plugins.upgrade() else { break };
                plugins.dispatch().await;
            }
        });
    }

    /// Starts each delivery that is due and not already under way. One plugin, library, hook,
    /// and user has at most one delivery at a time; different ones run side by side.
    pub async fn dispatch(self: &Arc<Plugins>) {
        let due = match self.db.read(|conn| due::all(conn, now_ms())).await {
            Ok(due) => due,
            Err(error) => return tracing::error!(%error, "cannot find hook events to deliver"),
        };
        let Ok(runtime) = self.runtime() else { return };
        for due in due {
            let key = (due.plugin.clone(), due.library, due.hook.name(), due.user);
            if !self.delivering.lock().expect("delivery lock").insert(key.clone()) {
                continue;
            }
            let plugins = self.clone();
            runtime.spawn(async move {
                plugins.deliver(due).await;
                plugins.delivering.lock().expect("delivery lock").remove(&key);
            });
        }
    }

    /// Delivers what is due to one plugin in one library, and records how it went.
    async fn deliver(&self, due: Due) {
        let (plugin, library, user) = (due.plugin.clone(), due.library, due.user);
        let (hook, position) = (due.hook, due.position);
        let read = self
            .db
            .read(move |conn| {
                let plugin_id = plugin.clone();
                let at = Due { hook, plugin: plugin_id, library, position, failures: 0, user };
                let event = due::event(conn, &at, now_ms())?;
                Ok((
                    event,
                    grants::manifest(conn, &plugin)?,
                    grants::granted(conn, &plugin, library)?,
                ))
            })
            .await;
        let ((next, event), manifest, permissions) = match read {
            Ok((event, Some(manifest), permissions)) => (event, manifest, permissions),
            Ok((_, None, _)) => return,
            Err(error) => return tracing::error!(%error, "cannot read hook events"),
        };
        // Nothing for the plugin, such as a batch of album changes alone, moves the position on.
        let (succeeded, summary) = match event {
            None => (true, String::new()),
            Some(event) => {
                let user = (due.user != 0).then_some(due.user);
                match self.run_in(&due.plugin, &manifest, library, user, permissions, event).await {
                    Ok((outcome, _)) => (outcome.ok, outcome.summary),
                    Err(error) => (false, error.to_string()),
                }
            }
        };
        let recorded =
            self.db.write(move |tx| record(tx, &due, next, succeeded, &summary, now_ms())).await;
        if let Err(error) = recorded {
            tracing::error!(%error, "cannot record a hook delivery");
        }
    }
}

/// Moves the position on to `next` after a success. After a failure it keeps the position and
/// waits longer each time before trying again, and disables the plugin in the library once
/// it has failed [`MAX_FAILURES`] times in a row, unless it is acting for one user.
fn record(
    conn: &Connection,
    due: &Due,
    next: i64,
    succeeded: bool,
    summary: &str,
    now: i64,
) -> rusqlite::Result<()> {
    let (plugin, library, hook, user) = (&due.plugin, due.library, due.hook.name(), due.user);
    if succeeded {
        conn.execute(
            "INSERT INTO plugin_cursors (plugin_id, library_id, hook, user_id, position) \
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (plugin_id, library_id, hook, user_id) \
             DO UPDATE SET position = excluded.position, failures = 0, retry_at = NULL",
            params![plugin, library, hook, user, next],
        )?;
        return Ok(());
    }
    let failures = due.failures + 1;
    let backoff = (BACKOFF_MS << (failures - 1).min(10)).min(MAX_BACKOFF_MS);
    conn.execute(
        "INSERT INTO plugin_cursors (plugin_id, library_id, hook, user_id, failures, retry_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (plugin_id, library_id, hook, user_id) \
         DO UPDATE SET failures = excluded.failures, retry_at = excluded.retry_at",
        params![plugin, library, hook, user, failures, now + backoff],
    )?;
    if failures >= MAX_FAILURES && user == 0 {
        let reason = format!("Stopped after failing {failures} times in a row: {summary}");
        conn.execute(
            "UPDATE plugin_libraries SET enabled = 0, disabled_reason = ?3 \
             WHERE plugin_id = ?1 AND library_id = ?2",
            params![plugin, library, reason],
        )?;
        tracing::warn!(plugin, library, hook, "disabled a plugin that kept failing");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
