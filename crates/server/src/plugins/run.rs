//! Running a plugin in a library, for Run now or for a hook: with exactly its grants there, on
//! the plugin runtime, then scanning what it touched and recording how it went.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use jewelcase_plugins::{Event, Grants, Manifest, Outcome, Permission};
use jewelcase_scanner::Trigger;
use rusqlite::{Connection, params};
use serde_json::{Map, Value};

use super::library::RunLibrary;
use super::{PluginError, Plugins, grants, personal, settings};
use crate::db::now_ms;

/// How a run went, as the admin sees it.
pub struct RunResult {
    pub ok: bool,
    pub summary: String,
    pub log: Vec<String>,
    /// Files it saved into the library.
    pub saved: usize,
    /// Whether a scan was queued to pick those files up.
    pub scanning: bool,
}

impl RunResult {
    fn failed(summary: String) -> RunResult {
        RunResult { ok: false, summary, log: Vec::new(), saved: 0, scanning: false }
    }
}

impl Plugins {
    /// Runs `id` once in each library it is enabled in, to do its whole job there.
    pub async fn run(&self, id: String) -> Result<RunResult, PluginError> {
        let plan = {
            let id = id.clone();
            self.db.read(move |conn| plan(conn, &id)).await?.ok_or(PluginError::NotFound)?
        };
        let (manifest, libraries) = plan;
        if libraries.is_empty() {
            return Ok(RunResult::failed(format!(
                "{} is not enabled in any library.",
                manifest.name
            )));
        }
        let mut result = RunResult {
            ok: true,
            summary: String::new(),
            log: Vec::new(),
            saved: 0,
            scanning: false,
        };
        for (library, permissions) in libraries {
            let missing = grants::missing(&manifest, &permissions);
            if !missing.is_empty() {
                let names: Vec<_> = missing.into_iter().map(Permission::title).collect();
                return Ok(RunResult::failed(format!("It still needs {}.", names.join(" and "))));
            }
            let (outcome, scanning) =
                self.run_in(&id, &manifest, library, None, permissions, Event::Run).await?;
            result.scanning |= scanning;
            result.ok &= outcome.ok;
            result.saved += outcome.written;
            result.log.extend(outcome.log);
            if !result.summary.is_empty() {
                result.summary.push(' ');
            }
            result.summary.push_str(&outcome.summary);
        }
        Ok(result)
    }

    /// Runs `id` in `library` with `permissions` to handle `event`, for `user` if it acts for
    /// one, with their personal settings beside the shared ones. Then it queues scans of what it
    /// touched, as the scanner is the only way into the library (`requirements/general.md`
    /// §3.2), and records the run for admins. Answers the outcome, and whether a scan was
    /// queued.
    pub(super) async fn run_in(
        &self,
        id: &str,
        manifest: &Manifest,
        library: i64,
        user: Option<i64>,
        permissions: Vec<Permission>,
        event: Event,
    ) -> Result<(Outcome, bool), PluginError> {
        let (plugin, schema) = (id.to_owned(), manifest.settings.clone());
        let personal_schema = manifest.personal_settings.clone().unwrap_or_default();
        let settings = self
            .db
            .read(move |conn| {
                let mut settings = settings::effective(conn, &plugin, schema.as_ref(), library)?;
                if let Some(user) = user {
                    let own = personal::values(conn, &plugin, user)?;
                    settings.extend(personal_schema.with_defaults(&own));
                }
                Ok(settings)
            })
            .await?;
        let outcome = self.execute(id, manifest, library, permissions, settings, event).await?;
        let scanning = self.scan(library, &outcome.touched).await;
        let (plugin, ok, summary) = (id.to_owned(), outcome.ok, outcome.summary.clone());
        self.db
            .write(move |tx| {
                tx.execute(
                    "UPDATE plugin_libraries SET last_run_at = ?3, last_run_ok = ?4, \
                     last_run_summary = ?5 WHERE plugin_id = ?1 AND library_id = ?2",
                    params![plugin, library, now_ms(), ok, summary],
                )
            })
            .await?;
        Ok((outcome, scanning))
    }

    /// Runs `id` in `library` with exactly these permissions and settings, on the plugin
    /// runtime, and nothing more.
    pub(super) async fn execute(
        &self,
        id: &str,
        manifest: &Manifest,
        library: i64,
        permissions: Vec<Permission>,
        settings: Map<String, Value>,
        event: Event,
    ) -> Result<Outcome, PluginError> {
        let (host, runtime) = (self.host()?, self.runtime()?);
        let grants =
            Grants { permissions, destinations: manifest.destinations().to_vec(), settings };
        let (path, db, plugin) = (self.path(id), self.db.clone(), id.to_owned());
        runtime
            .spawn(async move {
                host.run(&path, grants, RunLibrary::new(db, &plugin, library), event).await
            })
            .await
            .map_err(|e| PluginError::Internal(e.to_string()))
    }

    /// Queues a scan of each folder the paths in `touched` are in, and says whether any was
    /// queued. Queuing records the scan with a blocking write, which must not run on an async
    /// thread: there it would panic while holding the scanner's queue, and take that library's
    /// scanning down.
    pub(super) async fn scan(&self, library: i64, touched: &[PathBuf]) -> bool {
        let Some(scanner) = self.scanners.get(&library).cloned() else { return false };
        let folders: BTreeSet<PathBuf> =
            touched.iter().filter_map(|file| file.parent()).map(Path::to_path_buf).collect();
        tokio::task::spawn_blocking(move || {
            // Every folder is queued, so none is skipped because an earlier one succeeded.
            let queued = folders.iter().map(|folder| scanner.scan_folder(Trigger::Watch, folder));
            queued.fold(false, |any, scan| any | scan.is_some())
        })
        .await
        .unwrap_or(false)
    }
}

/// The manifest of `id`, and each library it is enabled in with what is granted there.
type Plan = (Manifest, Vec<(i64, Vec<Permission>)>);

fn plan(conn: &Connection, id: &str) -> rusqlite::Result<Option<Plan>> {
    let Some(manifest) = grants::manifest(conn, id)? else { return Ok(None) };
    let libraries: Vec<i64> = conn
        .prepare_cached(
            "SELECT library_id FROM plugin_libraries WHERE plugin_id = ?1 AND enabled \
             ORDER BY library_id",
        )?
        .query_map([id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let libraries = libraries
        .into_iter()
        .map(|library| Ok((library, grants::granted(conn, id, library)?)))
        .collect::<rusqlite::Result<_>>()?;
    Ok(Some((manifest, libraries)))
}
