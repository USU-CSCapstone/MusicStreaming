//! What each user sets for a plugin that acts for them, such as their own account on a
//! scrobbling service (`requirements/plugins.md` §4.1, §6): the `personalSettings` its manifest
//! declares. A user who has saved them has connected the plugin, and only then does it act for
//! them. Anyone may connect a plugin enabled in a library they reach; that grants it nothing the
//! admin did not.
//!
//! Sharing searches takes one step more (`requirements/search.md` §6): a connected user turns
//! it on for that plugin, and it is off until they do. Turning it off, or disconnecting, asks
//! the plugin to forget every search it was sent, in each library they reach
//! (`requirements/users.md` §7).

use jewelcase_plugins::{Manifest, Permission};
use rusqlite::{Connection, OptionalExtension, named_params, params};
use serde_json::{Map, Value};

use super::run::RunFor;
use super::settings::{self, SERVER, Saving};
use super::{PluginError, Plugins, grants};
use crate::api::Caller;
use crate::db::{DbError, Json, now_ms, reaches};

/// Whether `:user` (an admin if `:admin`) reaches a library the plugin `p` is enabled in.
const REACHED: &str = concat!(
    "EXISTS (SELECT 1 FROM plugin_libraries l WHERE l.plugin_id = p.id AND l.enabled AND ",
    reaches!(admin: ":admin", user: ":user", library: "l.library_id"),
    ")"
);

/// Asks the plugin `:id` to forget `:user`'s searches at `:at`, in each library it serves that
/// they (an admin if `:admin`) reach.
const STOPPED: &str = concat!(
    "INSERT INTO search_events (user_id, library_id, kind, plugin_id, created_at) \
     SELECT :user, l.library_id, 'stopped', :id, :at FROM plugin_libraries l \
     WHERE l.plugin_id = :id AND ",
    reaches!(admin: ":admin", user: ":user", library: "l.library_id")
);

/// A plugin the user can connect.
pub struct Connectable {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub connected: bool,
    /// Whether the user could share their searches with it ([`asks_for_searches`]).
    pub asks_for_searches: bool,
    pub shares_searches: bool,
}

/// Every plugin `caller` can connect: each with personal settings, enabled in a library they
/// reach, by name.
pub fn connectable(conn: &Connection, caller: Caller) -> rusqlite::Result<Vec<Connectable>> {
    conn.prepare_cached(&format!(
        "SELECT p.id, json_extract(p.manifest, '$.name') AS name, \
                json_extract(p.manifest, '$.description'), \
                c.user_id IS NOT NULL, c.searches_shared_at IS NOT NULL \
         FROM plugins p \
         LEFT JOIN plugin_connections c ON c.plugin_id = p.id AND c.user_id = :user \
         WHERE json_extract(p.manifest, '$.personalSettings') IS NOT NULL \
              AND {REACHED} ORDER BY name, p.id"
    ))?
    .query_map(named_params! { ":user": caller.user, ":admin": caller.admin }, |row| {
        Ok(Connectable {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            connected: row.get(3)?,
            asks_for_searches: false,
            shares_searches: row.get(4)?,
        })
    })?
    .collect::<rusqlite::Result<Vec<_>>>()?
    .into_iter()
    .map(|mut plugin| {
        plugin.asks_for_searches = asks_for_searches(conn, &plugin.id)?;
        Ok(plugin)
    })
    .collect()
}

/// Whether `id` can be sent its users' searches: it asks for the `searched` hook, and the admin
/// granted that and search activity. Without both, sharing would send nothing.
pub fn asks_for_searches(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let Some(manifest) = grants::manifest(conn, id)? else { return Ok(false) };
    let asks = manifest.permissions.iter().any(|r| r.permission == Permission::Searched);
    let granted = grants::granted(conn, id, SERVER)?;
    Ok(asks
        && [Permission::Searched, Permission::SearchActivity].iter().all(|p| granted.contains(p)))
}

/// Why turning sharing searches on or off failed.
#[derive(Debug)]
pub enum SharingError {
    /// No such plugin the user can connect.
    Unknown,
    /// They have not connected it.
    Unconnected,
    /// It cannot be sent searches ([`asks_for_searches`]).
    SearchesNotAllowed,
    /// The database failed.
    Db(DbError),
}

impl From<DbError> for SharingError {
    /// A database failure, as [`SharingError::Db`].
    fn from(error: DbError) -> SharingError {
        SharingError::Db(error)
    }
}

impl From<rusqlite::Error> for SharingError {
    /// A database error, as [`SharingError::Db`].
    fn from(error: rusqlite::Error) -> SharingError {
        SharingError::Db(error.into())
    }
}

/// Turns sharing `caller`'s searches with `id` on or off. On applies from now: nothing searched
/// before is sent. Off asks it to forget everything it was sent, in each library it serves that
/// they reach. A failure is a [`SharingError`] that says why.
pub fn set_search_sharing(
    conn: &Connection,
    id: &str,
    caller: Caller,
    sharing: bool,
) -> Result<(), SharingError> {
    if manifest(conn, id, caller)?.is_none() {
        return Err(SharingError::Unknown);
    }
    if !sharing {
        return Ok(stop_sharing(conn, id, caller)?);
    }
    let user = caller.user;
    let connected = conn
        .prepare_cached("SELECT 1 FROM plugin_connections WHERE plugin_id = ?1 AND user_id = ?2")?
        .query_row(params![id, user], |_| Ok(()))
        .optional()?
        .is_some();
    if !connected {
        return Err(SharingError::Unconnected);
    }
    if !asks_for_searches(conn, id)? {
        return Err(SharingError::SearchesNotAllowed);
    }
    conn.execute(
        "UPDATE plugin_connections SET searches_shared_at = ?3 \
         WHERE plugin_id = ?1 AND user_id = ?2 AND searches_shared_at IS NULL",
        params![id, user, now_ms()],
    )?;
    Ok(())
}

/// Stops sharing `caller`'s searches with `id`, if they were, and asks it to forget them in every
/// library it serves that they reach.
fn stop_sharing(conn: &Connection, id: &str, caller: Caller) -> rusqlite::Result<()> {
    let Caller { user, admin, .. } = caller;
    let stopped = conn.execute(
        "UPDATE plugin_connections SET searches_shared_at = NULL \
         WHERE plugin_id = ?1 AND user_id = ?2 AND searches_shared_at IS NOT NULL",
        params![id, user],
    )?;
    if stopped > 0 {
        let at = now_ms();
        let params = named_params! { ":id": id, ":user": user, ":admin": admin, ":at": at };
        conn.prepare_cached(STOPPED)?.execute(params)?;
    }
    Ok(())
}

/// The manifest of `id`, if `caller` can connect it.
pub fn manifest(conn: &Connection, id: &str, caller: Caller) -> rusqlite::Result<Option<Manifest>> {
    let Caller { user, admin, .. } = caller;
    let reached: bool = conn
        .prepare_cached(&format!("SELECT {REACHED} FROM plugins p WHERE p.id = :id"))?
        .query_row(named_params! { ":id": id, ":user": user, ":admin": admin }, |row| row.get(0))
        .optional()?
        .unwrap_or(false);
    let manifest = if reached { grants::manifest(conn, id)? } else { None };
    Ok(manifest.filter(|m| m.personal_settings.is_some()))
}

/// What `user` has set for `id`: nothing unless they have connected it. Stored values that
/// cannot be read are an error, so a plugin never runs without what the user set.
pub fn values(conn: &Connection, id: &str, user: i64) -> rusqlite::Result<Map<String, Value>> {
    let json: Option<Json<Map<String, Value>>> = conn
        .prepare_cached(
            "SELECT settings FROM plugin_connections WHERE plugin_id = ?1 AND user_id = ?2",
        )?
        .query_row(params![id, user], |row| row.get(0))
        .optional()?;
    Ok(json.map(|Json(values)| values).unwrap_or_default())
}

impl Plugins {
    /// Connects `caller` to `id`, or changes what they connected it with, from the values they
    /// `entered`, checked as an admin's settings are ([`Plugins::checked`]), with the shared
    /// settings beside them. A user's own settings must give every required one.
    pub async fn save_personal(
        &self,
        id: String,
        caller: Caller,
        entered: Map<String, Value>,
    ) -> Result<(), PluginError> {
        let user = caller.user;
        let saving = {
            let id = id.clone();
            self.db
                .try_read(move |conn| -> Result<_, PluginError> {
                    let manifest = manifest(conn, &id, caller)?.ok_or(PluginError::NotFound)?;
                    let schema = manifest.personal_settings.clone().unwrap_or_default();
                    let stored = values(conn, &id, user)?;
                    let beneath =
                        settings::effective(conn, &id, manifest.settings.as_ref(), SERVER)?;
                    let permissions = grants::granted(conn, &id, SERVER)?;
                    Ok(Saving { manifest, schema, stored, beneath, permissions })
                })
                .await?
        };
        let run_for = RunFor { library: SERVER, user: Some(user) };
        let values = self.checked(&id, run_for, saving, entered).await?;
        self.db.try_write(move |tx| connect(tx, &id, user, &values, now_ms())).await
    }
}

/// Connects `user` to `id` at `at` with `settings`, or replaces the settings they connected it
/// with, keeping when they connected and whether they share their searches. Refused as
/// [`PluginError::NotFound`], connecting nothing, if `id` is not installed, such as when it was
/// uninstalled meanwhile.
pub(super) fn connect(
    conn: &Connection,
    id: &str,
    user: i64,
    settings: &Map<String, Value>,
    at: i64,
) -> Result<(), PluginError> {
    let changed = conn.execute(
        "INSERT INTO plugin_connections (plugin_id, user_id, settings, connected_at) \
         SELECT id, ?2, ?3, ?4 FROM plugins WHERE id = ?1 \
         ON CONFLICT (plugin_id, user_id) DO UPDATE SET settings = excluded.settings",
        params![id, user, Json(settings), at],
    )?;
    if changed == 0 {
        return Err(PluginError::NotFound);
    }
    Ok(())
}

/// Disconnects `caller` from `id`: it stops being sent their searches, their settings are
/// forgotten, and it no longer acts for them.
pub fn disconnect(conn: &Connection, id: &str, caller: Caller) -> rusqlite::Result<()> {
    stop_sharing(conn, id, caller)?;
    conn.execute(
        "DELETE FROM plugin_connections WHERE plugin_id = ?1 AND user_id = ?2",
        params![id, caller.user],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
