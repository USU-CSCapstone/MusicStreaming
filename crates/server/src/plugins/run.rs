//! Running a plugin in a library, for Run now or for a hook: with exactly its grants there, on
//! the plugin runtime, then scanning what it touched and recording how it went.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jewelcase_plugins::{Event, Grants, Manifest, Outcome, Permission};
use jewelcase_scanner::{Depth, Scope, Trigger};
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::{Map, Value};

use super::library::RunLibrary;
use super::{PluginError, Plugins, grants, personal, rate_limits, settings};
use crate::db::now_ms;

/// The last-run summary of a run that failed on the server's side, whose cause goes only to the
/// log, so a database message never reaches the admin's page.
const SERVER_FAILED: &str = "the server could not complete the run";

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
    /// A run that did nothing, for the reason `summary` gives.
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
    /// Runs `id` once in each library it is enabled in, in its turn there, to do its whole job
    /// there. A library it is disabled in by the time its turn comes is passed over. The runs are
    /// scanned and recorded even if the caller stops waiting for them.
    pub async fn run(self: &Arc<Plugins>, id: String) -> Result<RunResult, PluginError> {
        let (manifest, libraries) = {
            let id = id.clone();
            self.db.try_read(move |conn| plan(conn, &id)).await?
        };
        let nowhere =
            RunResult::failed(format!("{} is not enabled in any library.", manifest.name));
        if libraries.is_empty() {
            return Ok(nowhere);
        }
        let plugins = self.clone();
        self.spawn(async move {
            let mut ran = false;
            let mut result = RunResult {
                ok: true,
                summary: String::new(),
                log: Vec::new(),
                saved: 0,
                scanning: false,
            };
            for library in libraries {
                let _turn = plugins.turns.take(&id, library).await;
                let plugin = id.clone();
                let granted = plugins.db.read(move |conn| grants::for_run(conn, &plugin, library));
                // Disabled there, or uninstalled, while it waited for its turn.
                let Some(permissions) = granted.await? else { continue };
                ran = true;
                let run_for = RunFor { library, user: None };
                let (outcome, scanning) =
                    plugins.run_in(&id, &manifest, run_for, permissions, Event::Run).await?;
                result.scanning |= scanning;
                result.ok &= outcome.result.is_ok();
                result.saved += outcome.written;
                if !result.summary.is_empty() {
                    result.summary.push(' ');
                }
                result.summary.push_str(&outcome.summary());
                result.log.extend(outcome.log);
            }
            Ok(if ran { result } else { nowhere })
        })
        .await?
    }

    /// Runs `id` in `run_for`'s library with `permissions` to handle `event`, for its user if it
    /// acts for one, with their personal settings beside the shared ones ([`Plugins::execute`]),
    /// and records its report as the library's last run for admins. An error on the server's side
    /// that kept it from running is logged, and recorded as [`SERVER_FAILED`]. A report that
    /// cannot be recorded is logged, and changes nothing it answers.
    /// Answers the outcome, and whether a scan of what it touched was queued.
    /// Called on the plugin runtime, in the plugin's turn in the library.
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
        let settings = self.db.read(move |conn| {
            let mut settings = settings::effective(conn, &plugin, schema.as_ref(), library)?;
            if let Some(user) = user {
                let own = personal::values(conn, &plugin, user)?;
                settings.extend(personal_schema.with_defaults(&own));
            }
            Ok(settings)
        });
        let ran = match settings.await {
            Ok(settings) => self.execute(id, manifest, run_for, permissions, settings, event).await,
            Err(error) => Err(error.into()),
        };
        let (ok, summary) = match &ran {
            Ok((outcome, _)) => (outcome.result.is_ok(), outcome.report()),
            Err(error) => {
                tracing::error!(plugin = id, library, %error, "a plugin run failed on the server");
                (false, SERVER_FAILED.to_owned())
            }
        };
        let plugin = id.to_owned();
        let recorded = self.db.write(move |tx| {
            tx.execute(
                "UPDATE plugin_libraries SET last_run_at = ?3, last_run_ok = ?4, \
                 last_run_summary = ?5 WHERE plugin_id = ?1 AND library_id = ?2",
                params![plugin, library, now_ms(), ok, summary],
            )
        });
        if let Err(error) = recorded.await {
            tracing::error!(plugin = id, library, %error, "cannot record a plugin's last run");
        }
        ran
    }

    /// Runs `id` in `run_for`'s library, for its user if it acts for one, with exactly these
    /// permissions and settings, and the host limits in force now, and nothing more. Then it
    /// queues a scan of what it touched, as the scanner is the only way into the library
    /// (`requirements/general.md` §3.2). What it was refused goes to the log for admins,
    /// whatever ran it. Answers the outcome, and whether a scan was queued. Called only on the
    /// plugin runtime, so plugin code never shares the threads serving requests, and in the
    /// plugin's turn in the library.
    pub(super) async fn execute(
        &self,
        id: &str,
        manifest: &Manifest,
        run_for: RunFor,
        permissions: Vec<Permission>,
        settings: Map<String, Value>,
        event: Event,
    ) -> Result<(Outcome, bool), PluginError> {
        let host = self.host()?;
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
        let library_access = RunLibrary::new(self.db.clone(), id, library);
        let outcome = host.run(&self.path(id), grants, library_access, event).await;
        if !outcome.refused.is_empty() {
            let refused = &outcome.refused;
            tracing::warn!(plugin = id, library, user, %refused, "refused a plugin");
        }
        let scanning = self.scan(library, &outcome.touched).await;
        Ok((outcome, scanning))
    }

    /// Queues one scan of every folder the paths in `touched` are in, within the library's
    /// roots, and says whether it was queued. Queuing records the scan with a blocking write,
    /// which must not run on an async thread: there it would panic while holding the scanner's
    /// queue, and take that library's scanning down.
    pub(super) async fn scan(&self, library: i64, touched: &[PathBuf]) -> bool {
        let Some(scanner) = self.scanners.get(&library).cloned() else { return false };
        let folders: BTreeSet<PathBuf> =
            touched.iter().filter_map(|file| file.parent()).map(Path::to_path_buf).collect();
        tokio::task::spawn_blocking(move || {
            let scopes: Vec<Scope> = folders
                .into_iter()
                .filter_map(|folder| {
                    let root = scanner.root_for(&folder)?;
                    Some(Scope::folder(root, folder, Depth::Subtree))
                })
                .collect();
            !scopes.is_empty() && scanner.request(Trigger::Watch, scopes).is_some()
        })
        .await
        .unwrap_or_else(|error| {
            tracing::error!(library, %error, "queuing a scan of what a plugin touched failed");
            false
        })
    }
}

/// The manifest of `id`, and each library it is enabled in.
type Plan = (Manifest, Vec<i64>);

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
    Ok((manifest, libraries))
}
