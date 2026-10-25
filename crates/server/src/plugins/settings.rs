//! What admins set for a plugin (`requirements/plugins.md` §6): server-wide, and per library
//! where a library's differ. A run gets the settings in effect: the library's own, else the
//! server-wide ones, else the defaults the manifest declares.

use std::sync::Arc;

use jewelcase_plugins::settings::Schema;
use jewelcase_plugins::{Event, Manifest};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value};

use super::run::RunFor;
use super::{PluginError, Plugins, grants};
use crate::db::Json;

/// The level of every library.
pub const SERVER: i64 = 0;

/// The values stored at `level`: [`SERVER`] or a library's ID. None are stored until an admin
/// saves some; stored values that cannot be read are an error, so a plugin never runs without
/// what the admin set.
pub fn level(conn: &Connection, id: &str, level: i64) -> rusqlite::Result<Map<String, Value>> {
    Ok(with_secrets(conn, id, level)?.0)
}

/// The values stored at `level`, as [`level`] reads them, and the names of those stored as
/// secrets ([`store`]).
pub fn with_secrets(
    conn: &Connection,
    id: &str,
    level: i64,
) -> rusqlite::Result<(Map<String, Value>, Vec<String>)> {
    let sql =
        "SELECT settings, secrets FROM plugin_settings WHERE plugin_id = ?1 AND library_id = ?2";
    let stored = conn
        .prepare_cached(sql)?
        .query_row(params![id, level], |row| {
            let (Json(values), Json(secrets)) = (row.get(0)?, row.get(1)?);
            Ok((values, secrets))
        })
        .optional()?;
    Ok(stored.unwrap_or_default())
}

/// The values set for `library`: its own over the server-wide ones. [`SERVER`] has only the
/// server-wide ones.
pub fn set_for(conn: &Connection, id: &str, library: i64) -> rusqlite::Result<Map<String, Value>> {
    let mut values = level(conn, id, SERVER)?;
    if library != SERVER {
        values.extend(level(conn, id, library)?);
    }
    Ok(values)
}

/// The settings in effect for a run in `library`, defaults included.
pub fn effective(
    conn: &Connection,
    id: &str,
    schema: Option<&Schema>,
    library: i64,
) -> rusqlite::Result<Map<String, Value>> {
    let set = set_for(conn, id, library)?;
    Ok(match schema {
        Some(schema) => schema.with_defaults(&set),
        None => set,
    })
}

/// Stores `values` as `id`'s settings at `level`, replacing any stored there, and notes which
/// of them are among `secrets`, the names the schema marks secret, so they stay hidden even
/// under a later manifest that no longer marks them.
pub fn store(
    conn: &Connection,
    id: &str,
    level: i64,
    values: &Map<String, Value>,
    secrets: &[String],
) -> rusqlite::Result<()> {
    let stored: Vec<&String> = secrets.iter().filter(|name| values.contains_key(*name)).collect();
    conn.execute(
        "INSERT INTO plugin_settings (plugin_id, library_id, settings, secrets) \
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT (plugin_id, library_id) \
         DO UPDATE SET settings = excluded.settings, secrets = excluded.secrets",
        params![id, level, Json(values), Json(stored)],
    )
    .map(drop)
}

/// The values to store when an admin saves `entered` at a level that held `stored`. A value
/// entered as `null` is cleared. A setting left out is cleared too, except a secret, which
/// keeps its value, since a form never shows one to send back.
pub fn merge(
    schema: &Schema,
    stored: &Map<String, Value>,
    entered: Map<String, Value>,
) -> Map<String, Value> {
    let secrets: Vec<&str> = schema.secrets().collect();
    let mut values: Map<String, Value> = stored
        .iter()
        .filter(|(name, _)| secrets.contains(&name.as_str()))
        .map(|(n, v)| (n.clone(), v.clone()))
        .collect();
    for (name, value) in entered {
        if value.is_null() {
            values.remove(&name);
        } else {
            values.insert(name, value);
        }
    }
    values
}

/// Settings about to be saved at one level: an admin's, server-wide or for a library, or one
/// user's own.
pub(super) struct Saving {
    /// The plugin's manifest, for its check run.
    pub manifest: Manifest,
    /// The settings saved there: the manifest's `settings` or `personalSettings`.
    pub schema: Schema,
    /// The values stored there now.
    pub stored: Map<String, Value>,
    /// What a run there gets beneath them: the server-wide settings under a library's, and the
    /// shared settings in effect beside a user's.
    pub beneath: Map<String, Value>,
}

impl Plugins {
    /// Saves settings an admin `entered` for `id` at `level`: [`SERVER`] or a library's ID, as
    /// [`Plugins::checked`] accepts them. Stored values at `level` that cannot be read are logged
    /// and saved over as if none were set, so an admin can mend them by entering every setting
    /// again.
    pub async fn save_settings(
        self: &Arc<Plugins>,
        id: String,
        level: i64,
        entered: Map<String, Value>,
    ) -> Result<(), PluginError> {
        let saving = {
            let id = id.clone();
            self.db
                .try_read(move |conn| -> Result<_, PluginError> {
                    let manifest = grants::manifest(conn, &id)?.ok_or(PluginError::NotFound)?;
                    let stored = match self::level(conn, &id, level) {
                        Err(error @ rusqlite::Error::FromSqlConversionFailure(..)) => {
                            tracing::warn!(
                                plugin = id,
                                level,
                                %error,
                                "stored settings cannot be read; the save replaces them"
                            );
                            Map::new()
                        }
                        stored => stored?,
                    };
                    // At the server level, `stored` is the server-wide settings.
                    let beneath =
                        if level == SERVER { Map::new() } else { self::level(conn, &id, SERVER)? };
                    let schema = manifest.settings.clone().unwrap_or_default();
                    Ok(Saving { manifest, schema, stored, beneath })
                })
                .await?
        };
        let run_for = RunFor { library: level, user: None };
        let secrets: Vec<String> = saving.schema.secrets().map(str::to_owned).collect();
        let values = self.checked(&id, run_for, saving, entered).await?;
        self.db.write(move |tx| store(tx, &id, level, &values, &secrets)).await?;
        Ok(())
    }

    /// The values to store when `entered` is saved for `id` over what `saving` holds ([`merge`]),
    /// once they are of the kinds and among the choices its schema declares, leave no required
    /// setting without a value, and pass the plugin's own check, run for `run_for` with them, what
    /// is beneath them, and the defaults, so a wrong key is caught as it is entered rather than at
    /// three in the morning (§6). The check runs in the plugin's turn in the library, or a user's
    /// in a turn of that user's, so no user runs more than one check of a plugin at once, with what
    /// the library grants, even where the plugin is disabled: the one run an admin's action makes
    /// there (`requirements/plugins.md` §5), so a wrong value is caught as it is entered. A user's
    /// check is always at the server level, where nothing is disabled. One uninstalled by its turn
    /// is a [`PluginError::NotFound`]; uninstalling does not wait for a user's turn. A caller that
    /// stops waiting for its turn leaves the queue; a check that has started goes on to its end,
    /// and what it touched is scanned, as after any run. Server-wide settings may leave a required
    /// one to each library, and a library's may take one from them; a user's must give every one
    /// themselves. Values that fail are a [`PluginError::SettingsInvalid`] that says why.
    pub(super) async fn checked(
        self: &Arc<Plugins>,
        id: &str,
        run_for: RunFor,
        saving: Saving,
        entered: Map<String, Value>,
    ) -> Result<Map<String, Value>, PluginError> {
        let Saving { manifest, schema, stored, beneath } = saving;
        let values = merge(&schema, &stored, entered);
        let problems = schema.check(&values);
        if !problems.is_empty() {
            return Err(PluginError::SettingsInvalid(format!("{}.", problems.join("; "))));
        }
        let mut set = beneath;
        set.extend(values.clone());
        let missing = match run_for {
            RunFor { user: Some(_), .. } => schema.missing(&values),
            RunFor { library: SERVER, .. } => Vec::new(),
            _ => schema.missing(&set),
        };
        if !missing.is_empty() {
            let missing = missing.join(" and ");
            return Err(PluginError::SettingsInvalid(format!("{missing} is required.")));
        }
        let candidate = schema.with_defaults(&set);
        // A user's check takes a turn of that user's, keyed by the negated user ID, which no
        // library ID or `SERVER` can be, so each user runs one check of a plugin at a time.
        let queue = run_for.user.map_or(run_for.library, |user| -user);
        let turn = self.turns.take(id, queue).await;
        let (plugins, id) = (self.clone(), id.to_owned());
        let checked = self.spawn(async move {
            let _turn = turn;
            let (plugin, library) = (id.clone(), run_for.library);
            let granted = plugins.db.try_read(move |conn| -> Result<_, PluginError> {
                // Uninstalled while it waited for its turn, or before a user's check began.
                grants::manifest(conn, &plugin)?.ok_or(PluginError::NotFound)?;
                Ok(grants::granted(conn, &plugin, library)?)
            });
            let permissions = granted.await?;
            let check = Event::CheckSettings;
            plugins.execute(&id, &manifest, run_for, permissions, candidate, check).await
        });
        let (outcome, _) = checked.await??;
        outcome.result.map_err(|error| PluginError::SettingsInvalid(error.to_string()))?;
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::testing::{database, install, manifest};
    use super::*;
    use crate::db::DbError;

    #[test]
    fn a_save_keeps_secrets_left_out_and_clears_the_rest() {
        let schema: Schema = serde_json::from_value(json!({ "properties": {
            "apiKey": { "type": "string", "writeOnly": true },
            "token": { "type": "string", "writeOnly": true },
            "mode": { "type": "string" },
            "limit": { "type": "integer" }
        } }))
        .unwrap();
        let stored = json!({ "apiKey": "k", "token": "t", "mode": "fast", "limit": 3 });
        let entered = json!({ "token": null, "limit": 5 });
        let merged =
            merge(&schema, stored.as_object().unwrap(), entered.as_object().unwrap().clone());
        assert_eq!(Value::Object(merged), json!({ "apiKey": "k", "limit": 5 }));
    }

    /// Stored values that cannot be read are an error rather than none, so a plugin never runs
    /// without the key an admin gave it. SQLite takes a number too large for serde as JSON.
    #[tokio::test]
    async fn values_that_cannot_be_read_are_an_error() {
        let (_temp, db) = database().await;
        install(&db, manifest("plugin", &[])).await;
        let unreadable = r#"{"apiKey": "k", "limit": 1e400}"#;
        db.write(move |tx| {
            tx.execute(
                "INSERT INTO plugin_settings (plugin_id, library_id, settings) \
                 VALUES ('plugin', 0, ?1)",
                [unreadable],
            )
        })
        .await
        .unwrap();
        let read = db.read(|conn| level(conn, "plugin", SERVER)).await;
        assert!(
            matches!(read, Err(DbError::Sqlite(rusqlite::Error::FromSqlConversionFailure(0, ..)))),
            "{read:?}"
        );
        assert!(db.read(|conn| level(conn, "plugin", 1)).await.unwrap().is_empty(), "none set");
    }
}
