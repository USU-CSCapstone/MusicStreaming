//! The spec's `Plugin`: what an installed plugin is, what it asks for, and what it has been
//! granted and where it is enabled, in every library on the server.

use jewelcase_plugins::{Manifest, Permission, PermissionRequest};
use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use super::super::Id;
use super::super::sql::timestamp;
use crate::plugins::grants;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plugin {
    id: String,
    name: String,
    version: String,
    api_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    homepage: Option<String>,
    source: Source,
    installed_at: String,
    updated_at: String,
    /// What it asks for.
    permissions: Vec<PermissionRequest>,
    /// Plugin-wide permissions granted.
    granted: Vec<Permission>,
    libraries: Vec<PluginLibrary>,
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
}

const SELECT: &str = concat!(
    "SELECT id, manifest, source_url, ",
    timestamp!("installed_at"),
    ", ",
    timestamp!("updated_at"),
    " FROM plugins"
);

/// Every installed plugin, by name.
pub fn all(conn: &Connection) -> rusqlite::Result<Vec<Plugin>> {
    let rows: Vec<Stored> =
        conn.prepare_cached(SELECT)?.query_map([], stored)?.collect::<rusqlite::Result<_>>()?;
    let mut plugins =
        rows.into_iter().map(|r| view(conn, r)).collect::<rusqlite::Result<Vec<_>>>()?;
    plugins.sort_by_key(|p| p.name.to_lowercase());
    Ok(plugins)
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<Plugin>> {
    let found = conn
        .prepare_cached(&format!("{SELECT} WHERE id = ?1"))?
        .query_row([id], stored)
        .optional()?;
    found.map(|r| view(conn, r)).transpose()
}

struct Stored {
    id: String,
    manifest: Manifest,
    source_url: Option<String>,
    installed_at: String,
    updated_at: String,
}

fn stored(row: &Row) -> rusqlite::Result<Stored> {
    let manifest: String = row.get(1)?;
    Ok(Stored {
        id: row.get(0)?,
        manifest: serde_json::from_str(&manifest).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?,
        source_url: row.get(2)?,
        installed_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

fn view(conn: &Connection, r: Stored) -> rusqlite::Result<Plugin> {
    let plugin_wide: Vec<Permission> = conn
        .prepare_cached(
            "SELECT permission FROM plugin_grants WHERE plugin_id = ?1 ORDER BY permission",
        )?
        .query_map([&r.id], |row| grants::permission(row.get(0)?))?
        .collect::<rusqlite::Result<_>>()?;
    let libraries: Vec<(i64, Option<bool>, Option<String>)> = conn
        .prepare_cached(
            "SELECT l.id, p.enabled, p.disabled_reason FROM libraries l \
             LEFT JOIN plugin_libraries p ON p.library_id = l.id AND p.plugin_id = ?1 \
             ORDER BY l.created_at, l.id",
        )?
        .query_map([&r.id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let libraries = libraries
        .into_iter()
        .map(|(library, enabled, disabled_reason)| {
            let all = grants::granted(conn, &r.id, library)?;
            Ok(PluginLibrary {
                library_id: Id(library),
                enabled: enabled.unwrap_or(false),
                auto_disabled: disabled_reason.is_some(),
                missing_required: grants::missing(&r.manifest, &all),
                granted: all.into_iter().filter(|p| p.per_library()).collect(),
                disabled_reason,
            })
        })
        .collect::<rusqlite::Result<_>>()?;
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
        updated_at: r.updated_at,
        permissions: m.permissions,
        granted: plugin_wide,
        libraries,
    })
}
