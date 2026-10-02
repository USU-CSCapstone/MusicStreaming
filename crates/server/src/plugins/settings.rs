//! What admins set for a plugin (`requirements/plugins.md` §6): server-wide, and per library
//! where a library's differ. A run gets the settings in effect: the library's own, else the
//! server-wide ones, else the defaults the manifest declares.

use jewelcase_plugins::Event;
use jewelcase_plugins::settings::Schema;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value};

use super::{PluginError, Plugins, grants};

/// The level of every library.
pub const SERVER: i64 = 0;

/// The values stored at `level`: [`SERVER`] or a library's ID.
pub fn level(conn: &Connection, id: &str, level: i64) -> rusqlite::Result<Map<String, Value>> {
    let json: Option<String> = conn
        .prepare_cached(
            "SELECT settings FROM plugin_settings WHERE plugin_id = ?1 AND library_id = ?2",
        )?
        .query_row(params![id, level], |row| row.get(0))
        .optional()?;
    Ok(json.and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default())
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
    Ok(schema.map_or(set.clone(), |schema| schema.with_defaults(&set)))
}

pub fn store(
    conn: &Connection,
    id: &str,
    level: i64,
    values: &Map<String, Value>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO plugin_settings (plugin_id, library_id, settings) VALUES (?1, ?2, ?3) \
         ON CONFLICT (plugin_id, library_id) DO UPDATE SET settings = excluded.settings",
        params![id, level, Value::Object(values.clone()).to_string()],
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

impl Plugins {
    /// Saves settings an admin `entered` for `id` at `level`: [`SERVER`] or a library's ID. They
    /// must be of the kinds and among the choices the manifest declares, leave no required
    /// setting without a value, and pass the plugin's own check, run with them, so a wrong key
    /// is caught as it is entered rather than at three in the morning (§6). Only then are they
    /// stored.
    pub async fn save_settings(
        &self,
        id: String,
        level: i64,
        entered: Map<String, Value>,
    ) -> Result<(), PluginError> {
        let read = {
            let id = id.clone();
            self.db
                .read(move |conn| {
                    let Some(manifest) = grants::manifest(conn, &id)? else { return Ok(None) };
                    let (own, server) =
                        (self::level(conn, &id, level)?, self::level(conn, &id, SERVER)?);
                    // At the server level, only what is granted to the plugin itself: there is
                    // no library 0 to have grants in.
                    let permissions = grants::granted(conn, &id, level)?;
                    Ok(Some((manifest, own, server, permissions)))
                })
                .await?
        };
        let (manifest, own, server, permissions) = read.ok_or(PluginError::NotFound)?;
        let schema = manifest.settings.clone().unwrap_or_default();
        let values = merge(&schema, &own, entered);
        let problems = schema.check(&values);
        if !problems.is_empty() {
            return Err(PluginError::SettingsInvalid(format!("{}.", problems.join("; "))));
        }
        let mut set = if level == SERVER { Map::new() } else { server };
        set.extend(values.clone());
        let missing = schema.missing(&set);
        // Server-wide settings may leave a required one to each library; a library's may not.
        if level != SERVER && !missing.is_empty() {
            return Err(PluginError::SettingsInvalid(format!(
                "{} is required.",
                missing.join(" and ")
            )));
        }
        let candidate = schema.with_defaults(&set);
        let outcome = self
            .execute(&id, &manifest, level, permissions, candidate, Event::CheckSettings)
            .await?;
        if !outcome.ok {
            return Err(PluginError::SettingsInvalid(outcome.summary));
        }
        self.db.write(move |tx| store(tx, &id, level, &values)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
}
