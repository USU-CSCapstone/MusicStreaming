//! What each plugin is granted and where it is enabled (`requirements/plugins.md` §4.2, §5).
//!
//! Nothing is granted implicitly. A grant for something a plugin never asked for, or in the
//! wrong scope, is ignored. A plugin cannot be enabled in a library until its required
//! permissions are granted there, and losing one disables it at once.

use jewelcase_plugins::{Manifest, Permission, PermissionRequest};
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::{Json, Text};

/// Why a library was disabled by the server rather than by the admin.
const REVOKED: &str = "A required permission was revoked.";

/// The manifest of the installed plugin `id`.
pub fn manifest(conn: &Connection, id: &str) -> rusqlite::Result<Option<Manifest>> {
    conn.prepare_cached("SELECT manifest FROM plugins WHERE id = ?1")?
        .query_row([id], |row| Ok(row.get::<_, Json<Manifest>>(0)?.0))
        .optional()
}

/// Everything granted to `id` for `library`: its library permissions there, and its own, which
/// are kept as library 0. For library 0, only its own.
pub fn granted(conn: &Connection, id: &str, library: i64) -> rusqlite::Result<Vec<Permission>> {
    conn.prepare_cached(
        "SELECT permission FROM plugin_grants WHERE plugin_id = ?1 AND library_id IN (0, ?2)",
    )?
    .query_map(params![id, library], |row| Ok(row.get::<_, Text<Permission>>(0)?.0))?
    .collect()
}

/// What a run of `id` in `library` is granted ([`granted`]), or `None` in a library it is
/// disabled in, where it runs nothing, as in every library once it is uninstalled. At library 0,
/// the server level, it is never `None`.
pub fn for_run(
    conn: &Connection,
    id: &str,
    library: i64,
) -> rusqlite::Result<Option<Vec<Permission>>> {
    let runs: bool = conn
        .prepare_cached(
            "SELECT ?2 = 0 OR EXISTS (SELECT 1 FROM plugin_libraries \
             WHERE plugin_id = ?1 AND library_id = ?2 AND enabled)",
        )?
        .query_row(params![id, library], |row| row.get(0))?;
    runs.then(|| granted(conn, id, library)).transpose()
}

/// The required permissions `granted` leaves out, in the manifest's order.
pub fn missing(manifest: &Manifest, granted: &[Permission]) -> Vec<Permission> {
    manifest.required().filter(|p| !granted.contains(p)).collect()
}

/// Replaces what `id` is granted: `plugin_wide` for itself, and `per_library` in each library
/// it names, each with the request `manifest` makes for it. A library that loses a required
/// permission is disabled.
pub fn set(
    conn: &Connection,
    id: &str,
    manifest: &Manifest,
    plugin_wide: &[Permission],
    per_library: &[(i64, Vec<Permission>)],
) -> rusqlite::Result<()> {
    // The permission and the request approved in granting it, if it is asked for in that scope.
    let approve = |p: &Permission, library: bool| {
        let request = manifest.permissions.iter().find(|r| r.permission == *p);
        let request = request.filter(|_| p.per_library() == library);
        request.map(|r| approved(r).map(|approved| (Text(*p), approved))).transpose()
    };
    conn.execute("DELETE FROM plugin_grants WHERE plugin_id = ?1 AND library_id = 0", [id])?;
    for p in plugin_wide {
        let Some((name, request)) = approve(p, false)? else { continue };
        conn.execute(
            "INSERT OR IGNORE INTO plugin_grants (plugin_id, library_id, permission, approved) \
             VALUES (?1, 0, ?2, ?3)",
            params![id, name, request],
        )?;
    }
    // Library 0 holds the plugin's own grants, so it is never replaced as a library.
    for (library, permissions) in per_library.iter().filter(|(library, _)| *library != 0) {
        conn.execute(
            "DELETE FROM plugin_grants WHERE plugin_id = ?1 AND library_id = ?2",
            params![id, library],
        )?;
        for p in permissions {
            let Some((name, request)) = approve(p, true)? else { continue };
            // A library that does not exist is skipped, as the spec's IDs select and never add.
            conn.execute(
                "INSERT OR IGNORE INTO plugin_grants (plugin_id, library_id, permission, approved) \
                 SELECT ?1, id, ?3, ?4 FROM libraries WHERE id = ?2",
                params![id, library, name, request],
            )?;
        }
    }
    let enabled: Vec<i64> = conn
        .prepare_cached("SELECT library_id FROM plugin_libraries WHERE plugin_id = ?1 AND enabled")?
        .query_map([id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for library in enabled {
        if !missing(manifest, &granted(conn, id, library)?).is_empty() {
            conn.execute(
                "UPDATE plugin_libraries SET enabled = 0, disabled_reason = ?3 \
                 WHERE plugin_id = ?1 AND library_id = ?2",
                params![id, library, REVOKED],
            )?;
        }
    }
    Ok(())
}

/// Enables or disables `id` in `library`. Enabling answers the required permissions still
/// missing there, if any, and changes nothing. Otherwise it gives the plugin's hooks there a
/// fresh start: the failures and outages that may have disabled it are cleared, and what waits
/// to be retried is due at once. Enabling it where it is already enabled does the same.
pub fn set_enabled(
    conn: &Connection,
    id: &str,
    manifest: &Manifest,
    library: i64,
    enabled: bool,
) -> rusqlite::Result<Vec<Permission>> {
    if enabled {
        let missing = missing(manifest, &granted(conn, id, library)?);
        if !missing.is_empty() {
            return Ok(missing);
        }
        conn.execute(
            "UPDATE plugin_cursors SET failures = 0, outages = 0, outage_since = NULL, \
             retry_at = NULL WHERE plugin_id = ?1 AND library_id = ?2",
            params![id, library],
        )?;
    }
    conn.execute(
        "INSERT INTO plugin_libraries (plugin_id, library_id, enabled, disabled_reason) \
         VALUES (?1, ?2, ?3, NULL) ON CONFLICT (plugin_id, library_id) \
         DO UPDATE SET enabled = excluded.enabled, disabled_reason = NULL",
        params![id, library, enabled],
    )?;
    Ok(Vec::new())
}

/// Keeps, of the grants a reinstalled plugin held before, only those whose request its
/// `manifest` makes again unchanged ([`approved`]), so a new version never widens what was
/// approved (`requirements/plugins.md` §4.2, §5). Its exemptions from host rate limits are kept
/// only with its `network` grant, whose request names the hosts and limits they relate to.
pub fn keep_requested(conn: &Connection, manifest: &Manifest) -> rusqlite::Result<()> {
    let requests: Vec<String> =
        manifest.permissions.iter().map(approved).collect::<rusqlite::Result<_>>()?;
    conn.execute(
        "DELETE FROM plugin_grants WHERE plugin_id = ?1 \
         AND approved NOT IN (SELECT value FROM json_each(?2))",
        params![manifest.id, Json(requests)],
    )?;
    conn.execute(
        "DELETE FROM plugin_rate_limit_exemptions WHERE plugin_id = ?1 AND NOT EXISTS \
         (SELECT 1 FROM plugin_grants WHERE plugin_id = ?1 AND permission = 'network')",
        [&manifest.id],
    )?;
    Ok(())
}

/// `request` as a grant records it and a reinstall compares it: the manifest's request, read
/// into its typed form and written back as compact JSON. So its keys always come in one order,
/// absent ones are left out, and rate limits are sorted by host, while lists keep the order the
/// manifest gives them and every value is compared exactly, the reason included. A request that
/// does not serialize is a conversion error.
fn approved(request: &PermissionRequest) -> rusqlite::Result<String> {
    serde_json::to_string(request)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

#[cfg(test)]
mod tests;
