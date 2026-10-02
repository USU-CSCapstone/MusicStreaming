//! What each user sets for a plugin that acts for them, such as their own account on a
//! scrobbling service (`requirements/plugins.md` §4.1, §6): the `personalSettings` its manifest
//! declares. A user who has saved them has connected the plugin, and only then does it act for
//! them. Anyone may connect a plugin enabled in a library they reach; that grants it nothing the
//! admin did not.

use jewelcase_plugins::{Event, Manifest};
use rusqlite::{Connection, OptionalExtension, named_params, params};
use serde_json::{Map, Value};

use super::settings::{self, SERVER, merge};
use super::{PluginError, Plugins, grants};
use crate::db::now_ms;

/// Whether `:user` (an admin if `:admin`) reaches a library the plugin `p` is enabled in.
const REACHED: &str = "EXISTS (SELECT 1 FROM plugin_libraries l WHERE l.plugin_id = p.id \
     AND l.enabled AND (:admin OR l.library_id IN \
         (SELECT library_id FROM library_access WHERE user_id = :user)))";

/// A plugin the user can connect.
pub struct Connectable {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub connected: bool,
}

/// Every plugin `user` can connect: each with personal settings, enabled in a library they
/// reach, by name.
pub fn connectable(
    conn: &Connection,
    user: i64,
    admin: bool,
) -> rusqlite::Result<Vec<Connectable>> {
    conn.prepare_cached(&format!(
        "SELECT p.id, json_extract(p.manifest, '$.name') AS name, \
                json_extract(p.manifest, '$.description'), \
                EXISTS (SELECT 1 FROM plugin_user_settings s \
                        WHERE s.plugin_id = p.id AND s.user_id = :user) \
         FROM plugins p WHERE json_extract(p.manifest, '$.personalSettings') IS NOT NULL \
              AND {REACHED} ORDER BY name, p.id"
    ))?
    .query_map(named_params! { ":user": user, ":admin": admin }, |row| {
        Ok(Connectable {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            connected: row.get(3)?,
        })
    })?
    .collect()
}

/// The manifest of `id`, if `user` can connect it.
pub fn manifest(
    conn: &Connection,
    id: &str,
    user: i64,
    admin: bool,
) -> rusqlite::Result<Option<Manifest>> {
    let reached: bool = conn
        .prepare_cached(&format!("SELECT {REACHED} FROM plugins p WHERE p.id = :id"))?
        .query_row(named_params! { ":id": id, ":user": user, ":admin": admin }, |row| row.get(0))
        .optional()?
        .unwrap_or(false);
    let manifest = if reached { grants::manifest(conn, id)? } else { None };
    Ok(manifest.filter(|m| m.personal_settings.is_some()))
}

/// What `user` has set for `id`: nothing unless they have connected it.
pub fn values(conn: &Connection, id: &str, user: i64) -> rusqlite::Result<Map<String, Value>> {
    let json: Option<String> = conn
        .prepare_cached(
            "SELECT settings FROM plugin_user_settings WHERE plugin_id = ?1 AND user_id = ?2",
        )?
        .query_row(params![id, user], |row| row.get(0))
        .optional()?;
    Ok(json.and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default())
}

impl Plugins {
    /// Connects `user` to `id`, or changes what they connected it with, from the values they
    /// `entered`. Checked as an admin's settings are: by kind and choice, for every required
    /// one, and then by the plugin itself, run with the server-wide settings and these.
    pub async fn save_personal(
        &self,
        id: String,
        user: i64,
        admin: bool,
        entered: Map<String, Value>,
    ) -> Result<(), PluginError> {
        let read = {
            let id = id.clone();
            self.db
                .read(move |conn| {
                    let Some(manifest) = manifest(conn, &id, user, admin)? else { return Ok(None) };
                    let shared =
                        settings::effective(conn, &id, manifest.settings.as_ref(), SERVER)?;
                    let stored = values(conn, &id, user)?;
                    Ok(Some((manifest, shared, stored, grants::granted(conn, &id, SERVER)?)))
                })
                .await?
        };
        let (manifest, mut candidate, stored, permissions) = read.ok_or(PluginError::NotFound)?;
        let schema = manifest.personal_settings.clone().unwrap_or_default();
        let values = merge(&schema, &stored, entered);
        let problems = schema.check(&values);
        if !problems.is_empty() {
            return Err(PluginError::SettingsInvalid(format!("{}.", problems.join("; "))));
        }
        let missing = schema.missing(&values);
        if !missing.is_empty() {
            let missing = missing.join(" and ");
            return Err(PluginError::SettingsInvalid(format!("{missing} is required.")));
        }
        candidate.extend(schema.with_defaults(&values));
        let outcome = self
            .execute(&id, &manifest, SERVER, permissions, candidate, Event::CheckSettings)
            .await?;
        if !outcome.ok {
            return Err(PluginError::SettingsInvalid(outcome.summary));
        }
        let json = Value::Object(values).to_string();
        self.db
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO plugin_user_settings (plugin_id, user_id, settings, connected_at) \
                     VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT (plugin_id, user_id) DO UPDATE SET settings = excluded.settings",
                    params![id, user, json, now_ms()],
                )
            })
            .await?;
        Ok(())
    }
}

/// Disconnects `user` from `id`: their settings are forgotten, and it no longer acts for them.
pub fn disconnect(conn: &Connection, id: &str, user: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM plugin_user_settings WHERE plugin_id = ?1 AND user_id = ?2",
        params![id, user],
    )
    .map(drop)
}

#[cfg(test)]
mod tests;
