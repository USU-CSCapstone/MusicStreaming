//! The tag types plugins added, which a scan reads with the built-in ones
//! (`design/database.md` §3, `design/scanning.md` §4).

use jewelcase_scanner::AddedTagType;

use super::SqliteStore;
use crate::db::DbError;

/// Every type a plugin added, oldest first.
pub fn added(store: &SqliteStore) -> Result<Vec<AddedTagType>, DbError> {
    store.db.read_blocking(|conn| {
        conn.prepare_cached(
            "SELECT key, field, since FROM tag_types WHERE plugin_id IS NOT NULL ORDER BY since, key",
        )?
        .query_map([], |row| {
            Ok(AddedTagType { key: row.get(0)?, field: row.get(1)?, since: row.get(2)? })
        })?
        .collect()
    })
}
