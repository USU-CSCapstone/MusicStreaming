//! Running a plugin in a library, for Run now or for a hook: with exactly its grants there, on
//! the plugin runtime, then scanning what it touched and recording how it went.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use jewelcase_plugins::{Event, Grants, Manifest, Outcome, Permission};
use jewelcase_scanner::Trigger;
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::{Map, Value};

use super::library::RunLibrary;
use super::{PluginError, Plugins, grants, personal, rate_limits, settings, unexpected};
use crate::db::now_ms;

/// How a run went, as the admin sees it: the spec's `PluginRunResult`.
#[derive(Serialize)]
pub struct RunResult {
    pub ok: bool,
    pub summary: String,
    pub log: Vec<String>,
    /// Files it saved into the library.
    pub saved: usize,
    /// Whether the scanner will pick those up: a scan of their folders is queued.
    #[serde(rename = "scannerRunning")]
    pub scanning: bool,
}

impl RunResult {
    fn failed(summary: String) -> RunResult {
        RunResult { ok: false, summary, log: Vec::new(), saved: 0, scanning: false }
    }
}

/// Where a run is, and who for: a library, and the user it acts for if it acts for one.
#[derive(Clone, Copy)]
pub(super) struct RunFor {
    pub library: i64,
    pub user: Option<i64>,
}

impl Plugins {
    /// Runs `id` once in each library it is enabled in, to do its whole job there.
    pub async fn run(&self, id: String) -> Result<RunResult, PluginError> {
        let (manifest, libraries) = {
            let id = id.clone();
            self.db.try_read(move |conn| plan(conn, &id)).await?
        };
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
            let (outcome, scanning) = self
                .run_in(&id, &manifest, RunFor { library, user: None }, permissions, Event::Run)
                .await?;
            result.scanning |= scanning;
            result.ok &= outcome.result.is_ok();
            result.saved += outcome.written;
            if !result.summary.is_empty() {
                result.summary.push(' ');
            }
            result.summary.push_str(&outcome.summary());
            result.log.extend(outcome.log);
        }
        Ok(result)
    }

    /// Runs `id` in `run_for`'s library with `permissions` to handle `event`, for its user if it
    /// acts for one, with their personal settings beside the shared ones. Then it queues scans
    /// of what it touched, as the scanner is the only way into the library
    /// (`requirements/general.md` §3.2), and records its report as the library's last run for
    /// admins. Answers the outcome, and whether a scan was queued.
    pub(super) async fn run_in(
        &self,
        id: &str,
        manifest: &Manifest,
        run_for: RunFor,
        permissions: Vec<Permission>,
        event: Event,
    ) -> Result<(Outcome, bool), PluginError> {
        let RunFor { library, user } = run_for;
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
        let outcome = self.execute(id, manifest, run_for, permissions, settings, event).await?;
        let scanning = self.scan(library, &outcome.touched).await;
        let (plugin, ok, summary) = (id.to_owned(), outcome.result.is_ok(), outcome.report());
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

    /// Runs `id` in `run_for`'s library, for its user if it acts for one, with exactly these
    /// permissions and settings, and the host limits in force now, on the plugin runtime, and
    /// nothing more. What it was refused goes to the log for admins, whatever ran it.
    pub(super) async fn execute(
        &self,
        id: &str,
        manifest: &Manifest,
        run_for: RunFor,
        permissions: Vec<Permission>,
        settings: Map<String, Value>,
        event: Event,
    ) -> Result<Outcome, PluginError> {
        let (host, runtime) = (self.host()?, self.runtime()?);
        let plugin = id.to_owned();
        let host_limits = self.db.read(move |conn| rate_limits::for_run(conn, &plugin)).await?;
        let RunFor { library, user } = run_for;
        let grants = Grants {
            plugin: id.to_owned(),
            library,
            user,
            permissions,
            destinations: manifest.destinations().to_vec(),
            rate_limits: manifest.rate_limits(),
            host_limits,
            settings,
        };
        let (path, db, plugin) = (self.path(id), self.db.clone(), id.to_owned());
        let outcome = runtime
            .spawn(async move {
                host.run(&path, grants, RunLibrary::new(db, &plugin, library), event).await
            })
            .await
            .map_err(unexpected)?;
        if !outcome.refused.is_empty() {
            let refused = &outcome.refused;
            tracing::warn!(plugin = id, library, user, %refused, "refused a plugin");
        }
        Ok(outcome)
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

/// The [`Plan`] for running `id`, or [`PluginError::NotFound`] if it is not installed.
fn plan(conn: &Connection, id: &str) -> Result<Plan, PluginError> {
    let manifest = grants::manifest(conn, id)?.ok_or(PluginError::NotFound)?;
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
    Ok((manifest, libraries))
}
