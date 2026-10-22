//! Rate limits that every plugin's requests to a host keep to (`requirements/plugins.md` §7),
//! and the plugins an admin exempted from them.
//!
//! A plugin's `"per": "host"` limit is in force while it is installed, granted `network` and
//! `hostRateLimits`, and enabled in a library; where several plugins set one for a host, the
//! strictest wins. A run reads them as it starts, so a change takes effect from each plugin's
//! next run, as grants do.

use std::collections::BTreeMap;
use std::time::Duration;

use jewelcase_plugins::{Manifest, Per, RateLimit};
use rusqlite::{Connection, params};

use super::{PluginError, Plugins, grants};
use crate::db::Json;

/// The limit in force for one host.
pub struct HostLimit {
    /// Lowercase.
    pub host: String,
    /// The strictest declared, as its manifest gives it, such as `"1/s"`.
    pub limit: String,
    /// The gap that keeps between requests.
    pub gap: Duration,
    /// The plugins that declared it that strict, by ID.
    pub set_by: Vec<String>,
}

/// Every host limit in force, by host.
pub fn in_force(conn: &Connection) -> rusqlite::Result<Vec<HostLimit>> {
    // Grants outlive an uninstall; only an installed plugin's limits are in force.
    let setters: Vec<(String, rusqlite::Result<Json<Manifest>>)> = conn
        .prepare_cached(
            "SELECT p.id, p.manifest FROM plugins p JOIN plugin_grants g ON g.plugin_id = p.id \
             WHERE g.permission = 'hostRateLimits' \
             AND EXISTS (SELECT 1 FROM plugin_grants n \
                         WHERE n.plugin_id = p.id AND n.permission = 'network') \
             AND EXISTS (SELECT 1 FROM plugin_libraries l \
                         WHERE l.plugin_id = p.id AND l.enabled) \
             ORDER BY p.id",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1))))?
        .collect::<rusqlite::Result<_>>()?;
    let mut limits = BTreeMap::<String, HostLimit>::new();
    for (id, manifest) in setters {
        // Every run reads these as it starts, so a manifest that cannot be read is left out, and
        // logged for the admin, rather than stopping every plugin (`requirements/plugins.md` §11).
        let manifest = match manifest {
            Ok(Json(manifest)) => manifest,
            Err(error) => {
                tracing::warn!(
                    plugin = id,
                    %error,
                    "cannot read a manifest; its host limits are left out"
                );
                continue;
            }
        };
        for declared in manifest.rate_limits().into_iter().filter(|l| l.per == Per::Host) {
            match limits.get_mut(&declared.host) {
                Some(kept) if kept.gap > declared.gap => {}
                Some(kept) if kept.gap == declared.gap => kept.set_by.push(id.clone()),
                // The first for this host, or stricter than any before it.
                _ => {
                    let set_by = vec![id.clone()];
                    let RateLimit { host, limit, gap, .. } = declared;
                    let limit = HostLimit { host, limit, gap, set_by };
                    limits.insert(limit.host.clone(), limit);
                }
            }
        }
    }
    Ok(limits.into_values().collect())
}

/// The host limits a run of `id` keeps to: every one in force, but those it is exempt from.
pub fn for_run(conn: &Connection, id: &str) -> rusqlite::Result<Vec<(String, Duration)>> {
    let exempt = exemptions(conn, id)?;
    let limits = in_force(conn)?.into_iter().filter(|l| !exempt.contains(&l.host));
    Ok(limits.map(|l| (l.host, l.gap)).collect())
}

/// The hosts whose limits for every plugin `id` is exempt from.
pub fn exemptions(conn: &Connection, id: &str) -> rusqlite::Result<Vec<String>> {
    conn.prepare_cached(
        "SELECT host FROM plugin_rate_limit_exemptions WHERE plugin_id = ?1 ORDER BY host",
    )?
    .query_map([id], |row| row.get(0))?
    .collect()
}

impl Plugins {
    /// Exempts plugin `id` from the limit for every plugin on `host`, a lowercase host name,
    /// which need not have a limit in force now.
    pub async fn add_exemption(&self, id: String, host: String) -> Result<(), PluginError> {
        self.change_exemption(
            id,
            host,
            "INSERT OR IGNORE INTO plugin_rate_limit_exemptions (plugin_id, host) VALUES (?1, ?2)",
        )
        .await
    }

    /// Ends plugin `id`'s exemption from the limit for every plugin on `host`, if it had one.
    pub async fn remove_exemption(&self, id: String, host: String) -> Result<(), PluginError> {
        self.change_exemption(
            id,
            host,
            "DELETE FROM plugin_rate_limit_exemptions WHERE plugin_id = ?1 AND host = ?2",
        )
        .await
    }

    /// Runs `sql` on `id` and `host`, if `id` is installed.
    async fn change_exemption(
        &self,
        id: String,
        host: String,
        sql: &'static str,
    ) -> Result<(), PluginError> {
        self.db
            .try_write(move |tx| {
                grants::manifest(tx, &id)?.ok_or(PluginError::NotFound)?;
                tx.execute(sql, params![id, host])?;
                Ok(())
            })
            .await
    }
}

#[cfg(test)]
mod tests;
