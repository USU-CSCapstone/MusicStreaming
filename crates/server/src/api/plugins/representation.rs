//! The spec's `Plugin`: what an installed plugin is, what it asks for, and what it has been
//! granted and where it is enabled, in every library on the server.

use jewelcase_plugins::{Manifest, Permission, PermissionRequest, reaches};
use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use super::super::Id;
use super::super::time::serialize_rfc3339;
use crate::db::{Json, Text};
use crate::plugins::grants;
use crate::plugins::rate_limits::{self, HostLimit};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plugin {
    id: String,
    name: String,
    version: String,
    /// The contract it was built against, as the manifest gives it: `"0.1"`.
    api_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    homepage: Option<String>,
    source: Source,
    #[serde(serialize_with = "serialize_rfc3339")]
    installed_at: i64,
    /// What it asks for.
    permissions: Vec<PermissionRequest>,
    /// Plugin-wide permissions granted.
    granted: Vec<Permission>,
    /// Whether it declares settings an admin can set.
    has_settings: bool,
    libraries: Vec<PluginLibrary>,
    /// The limits in force for every plugin at the hosts it can reach.
    host_limits: Vec<PluginHostLimit>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginHostLimit {
    host: String,
    /// The strictest any plugin set, such as `"1/s"`.
    limit: String,
    /// The plugins that set it.
    set_by: Vec<String>,
    exempt: bool,
}

#[derive(Serialize)]
struct Source {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginLibrary {
    library_id: Id,
    enabled: bool,
    /// Disabled by the server, such as when a required permission was revoked.
    auto_disabled: bool,
    disabled_reason: Option<String>,
    /// Library permissions granted here.
    granted: Vec<Permission>,
    /// Required permissions not granted here: while any remain, it cannot be enabled.
    missing_required: Vec<Permission>,
    /// Its last run here, by Run now or a hook.
    last_run: Option<LastRun>,
}

#[derive(Serialize)]
struct LastRun {
    #[serde(serialize_with = "serialize_rfc3339")]
    at: i64,
    ok: bool,
    summary: String,
}

const SELECT: &str = "SELECT id, manifest, source_url, installed_at FROM plugins";

/// A library, and the plugin's standing in it if it was ever enabled or disabled there.
struct LibraryRow {
    library: i64,
    enabled: Option<bool>,
    disabled_reason: Option<String>,
    last_run: Option<LastRun>,
}

const LIBRARIES: &str = "SELECT l.id, p.enabled, p.disabled_reason, p.last_run_at, p.last_run_ok, \
     p.last_run_summary FROM libraries l \
     LEFT JOIN plugin_libraries p ON p.library_id = l.id AND p.plugin_id = ?1 \
     ORDER BY l.created_at, l.id";

/// Every installed plugin, by name.
pub fn all(conn: &Connection) -> rusqlite::Result<Vec<Plugin>> {
    let rows: Vec<Stored> =
        conn.prepare_cached(SELECT)?.query_map([], stored)?.collect::<rusqlite::Result<_>>()?;
    let limits = rate_limits::in_force(conn)?;
    let mut plugins =
        rows.into_iter().map(|r| view(conn, r, &limits)).collect::<rusqlite::Result<Vec<_>>>()?;
    plugins.sort_by_key(|p| p.name.to_lowercase());
    Ok(plugins)
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<Plugin>> {
    let found = conn
        .prepare_cached(&format!("{SELECT} WHERE id = ?1"))?
        .query_row([id], stored)
        .optional()?;
    found.map(|r| view(conn, r, &rate_limits::in_force(conn)?)).transpose()
}

struct Stored {
    id: String,
    manifest: Manifest,
    source_url: Option<String>,
    installed_at: i64,
}

/// The plugin in a row of [`SELECT`]. A manifest that cannot be read is an error.
fn stored(row: &Row) -> rusqlite::Result<Stored> {
    Ok(Stored {
        id: row.get(0)?,
        manifest: row.get::<_, Json<Manifest>>(1)?.0,
        source_url: row.get(2)?,
        installed_at: row.get(3)?,
    })
}

/// The spec's `Plugin` for `r`: its grants and standing in every library, and the host `limits`
/// that apply to the hosts it reaches.
fn view(conn: &Connection, r: Stored, limits: &[HostLimit]) -> rusqlite::Result<Plugin> {
    let plugin_wide: Vec<Permission> = conn
        .prepare_cached(
            "SELECT permission FROM plugin_grants WHERE plugin_id = ?1 AND library_id = 0 \
             ORDER BY permission",
        )?
        .query_map([&r.id], |row| Ok(row.get::<_, Text<Permission>>(0)?.0))?
        .collect::<rusqlite::Result<_>>()?;
    let rows: Vec<LibraryRow> = conn
        .prepare_cached(LIBRARIES)?
        .query_map([&r.id], |row| {
            let last_run = match row.get(3)? {
                Some(at) => Some(LastRun { at, ok: row.get(4)?, summary: row.get(5)? }),
                None => None,
            };
            Ok(LibraryRow {
                library: row.get(0)?,
                enabled: row.get(1)?,
                disabled_reason: row.get(2)?,
                last_run,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let libraries = rows
        .into_iter()
        .map(|LibraryRow { library, enabled, disabled_reason, last_run }| {
            let all = grants::granted(conn, &r.id, library)?;
            Ok(PluginLibrary {
                library_id: Id(library),
                enabled: enabled.unwrap_or(false),
                auto_disabled: disabled_reason.is_some(),
                missing_required: grants::missing(&r.manifest, &all),
                granted: all.into_iter().filter(|p| p.per_library()).collect(),
                disabled_reason,
                last_run,
            })
        })
        .collect::<rusqlite::Result<_>>()?;
    let exemptions = rate_limits::exemptions(conn, &r.id)?;
    let destinations = r.manifest.destinations();
    let host_limits = limits
        .iter()
        .filter(|l| reaches(&l.host, destinations))
        .map(|l| PluginHostLimit {
            host: l.host.clone(),
            limit: l.limit.clone(),
            set_by: l.set_by.clone(),
            exempt: exemptions.contains(&l.host),
        })
        .collect();
    let m = r.manifest;
    Ok(Plugin {
        id: r.id,
        name: m.name,
        version: m.version,
        api_version: m.api_version,
        description: m.description,
        author: m.author,
        homepage: m.homepage,
        source: match r.source_url {
            Some(url) => Source { kind: "url", url: Some(url) },
            None => Source { kind: "file", url: None },
        },
        installed_at: r.installed_at,
        permissions: m.permissions,
        has_settings: m.settings.as_ref().is_some_and(|s| !s.properties.is_empty()),
        granted: plugin_wide,
        libraries,
        host_limits,
    })
}
