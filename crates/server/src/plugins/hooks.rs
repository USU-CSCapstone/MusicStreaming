//! Hooks: running plugins when something happens, rather than when an admin asks
//! (`requirements/plugins.md` §8).
//!
//! `tracksChanged` reads the library change feed (`0004_feeds.sql`), which records every
//! track added, changed, or removed in the same transaction as the change. Each plugin and
//! library pair keeps its own position in it (`plugin_cursors`), which moves only once the
//! plugin has handled a batch. A failure is retried later from the same place, so nothing is
//! lost to a crash, a restart, or a service that was briefly down. One that keeps failing is
//! disabled in that library, and the admin is told why (§11).

use std::sync::{Arc, Weak};
use std::time::Duration;

use jewelcase_plugins::{Event, TracksChanged};
use rusqlite::{Connection, params};

use super::{Plugins, grants};
use crate::db::now_ms;

/// How often the dispatcher looks for changes to deliver.
const POLL: Duration = Duration::from_secs(5);
/// Changes delivered in one event. Small enough that a plugin making a request per track
/// finishes a batch well within a run's time limit.
const BATCH: i64 = 100;
/// Failures in a row before a plugin is disabled in a library.
const MAX_FAILURES: i64 = 5;
/// The wait after the first failure, doubling after each until [`MAX_BACKOFF_MS`].
const BACKOFF_MS: i64 = 60_000;
const MAX_BACKOFF_MS: i64 = 60 * 60_000;

/// A plugin and library pair with changes waiting, and how far delivery has got.
struct Due {
    plugin: String,
    library: i64,
    position: i64,
    failures: i64,
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

    /// Starts a delivery to each pair with changes waiting that has none running already.
    pub async fn dispatch(self: &Arc<Plugins>) {
        let due = match self.db.read(|conn| due(conn, now_ms())).await {
            Ok(due) => due,
            Err(error) => return tracing::error!(%error, "cannot find hook events to deliver"),
        };
        let Ok(runtime) = self.runtime() else { return };
        for due in due {
            let pair = (due.plugin.clone(), due.library);
            if !self.delivering.lock().expect("delivery lock").insert(pair.clone()) {
                continue;
            }
            let plugins = self.clone();
            runtime.spawn(async move {
                plugins.deliver(due).await;
                plugins.delivering.lock().expect("delivery lock").remove(&pair);
            });
        }
    }

    /// Delivers the next batch of changes to one pair, and records how it went.
    async fn deliver(&self, due: Due) {
        let (plugin, library, position) = (due.plugin.clone(), due.library, due.position);
        let read = self
            .db
            .read(move |conn| {
                let batch = batch(conn, library, position)?;
                Ok((
                    batch,
                    grants::manifest(conn, &plugin)?,
                    grants::granted(conn, &plugin, library)?,
                ))
            })
            .await;
        let ((next, changes), manifest, permissions) = match read {
            Ok((batch, Some(manifest), permissions)) => (batch, manifest, permissions),
            Ok((_, None, _)) => return,
            Err(error) => return tracing::error!(%error, "cannot read hook events"),
        };
        // A batch of albums and artists alone moves the position on, and runs nothing.
        let (succeeded, summary) = if changes.changed.is_empty() && changes.removed.is_empty() {
            (true, String::new())
        } else {
            let event = Event::TracksChanged(changes);
            match self.run_in(&due.plugin, &manifest, due.library, permissions, event).await {
                Ok((outcome, _)) => (outcome.ok, outcome.summary),
                Err(error) => (false, error.to_string()),
            }
        };
        let (plugin, library, failures) = (due.plugin, due.library, due.failures);
        let recorded = self
            .db
            .write(move |tx| {
                record(tx, &plugin, library, next, succeeded, failures, &summary, now_ms())
            })
            .await;
        if let Err(error) = recorded {
            tracing::error!(%error, "cannot record a hook delivery");
        }
    }
}

/// The pairs the tracksChanged hook should deliver to now: the plugin enabled in the library
/// with the hook and read access granted there, not waiting to retry, and with changes past
/// its position.
fn due(conn: &Connection, now: i64) -> rusqlite::Result<Vec<Due>> {
    conn.prepare_cached(
        "SELECT p.plugin_id, p.library_id, coalesce(c.position, 0), coalesce(c.failures, 0) \
         FROM plugin_libraries p \
         JOIN plugin_library_grants hook ON hook.plugin_id = p.plugin_id \
              AND hook.library_id = p.library_id AND hook.permission = 'tracksChanged' \
         JOIN plugin_library_grants reads ON reads.plugin_id = p.plugin_id \
              AND reads.library_id = p.library_id AND reads.permission = 'libraryRead' \
         LEFT JOIN plugin_cursors c ON c.plugin_id = p.plugin_id AND c.library_id = p.library_id \
              AND c.hook = 'tracksChanged' \
         WHERE p.enabled AND (c.retry_at IS NULL OR c.retry_at <= ?1) \
           AND EXISTS (SELECT 1 FROM library_changes f \
                       WHERE f.library_id = p.library_id AND f.seq > coalesce(c.position, 0))",
    )?
    .query_map([now], |row| {
        Ok(Due {
            plugin: row.get(0)?,
            library: row.get(1)?,
            position: row.get(2)?,
            failures: row.get(3)?,
        })
    })?
    .collect()
}

/// The next batch of changes to `library` after `position`: the position after it, and the
/// tracks it names.
fn batch(conn: &Connection, library: i64, position: i64) -> rusqlite::Result<(i64, TracksChanged)> {
    let rows: Vec<(i64, String, i64, String)> = conn
        .prepare_cached(
            "SELECT seq, entity_type, entity_id, op FROM library_changes \
             WHERE library_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?
        .query_map(params![library, position, BATCH], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let next = rows.last().map_or(position, |(seq, ..)| *seq);
    let mut changes = TracksChanged { changed: Vec::new(), removed: Vec::new() };
    for (_, entity, id, op) in rows {
        match (entity.as_str(), op.as_str()) {
            ("track", "delete") => changes.removed.push(id as u64),
            ("track", _) => changes.changed.push(id as u64),
            _ => {}
        }
    }
    Ok((next, changes))
}

/// Moves the pair's position on after a success. After a failure it keeps the position and
/// waits longer each time before trying again, and disables the plugin in the library once
/// it has failed [`MAX_FAILURES`] times in a row.
#[allow(clippy::too_many_arguments)]
fn record(
    conn: &Connection,
    plugin: &str,
    library: i64,
    next: i64,
    succeeded: bool,
    failures: i64,
    summary: &str,
    now: i64,
) -> rusqlite::Result<()> {
    if succeeded {
        conn.execute(
            "INSERT INTO plugin_cursors (plugin_id, library_id, hook, position) \
             VALUES (?1, ?2, 'tracksChanged', ?3) ON CONFLICT (plugin_id, library_id, hook) \
             DO UPDATE SET position = excluded.position, failures = 0, retry_at = NULL",
            params![plugin, library, next],
        )?;
        return Ok(());
    }
    let failures = failures + 1;
    let backoff = (BACKOFF_MS << (failures - 1).min(10)).min(MAX_BACKOFF_MS);
    conn.execute(
        "INSERT INTO plugin_cursors (plugin_id, library_id, hook, failures, retry_at) \
         VALUES (?1, ?2, 'tracksChanged', ?3, ?4) ON CONFLICT (plugin_id, library_id, hook) \
         DO UPDATE SET failures = excluded.failures, retry_at = excluded.retry_at",
        params![plugin, library, failures, now + backoff],
    )?;
    if failures >= MAX_FAILURES {
        let reason = format!("Stopped after failing {failures} times in a row: {summary}");
        conn.execute(
            "UPDATE plugin_libraries SET enabled = 0, disabled_reason = ?3 \
             WHERE plugin_id = ?1 AND library_id = ?2",
            params![plugin, library, reason],
        )?;
        tracing::warn!(plugin, library, "disabled a plugin that kept failing");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
