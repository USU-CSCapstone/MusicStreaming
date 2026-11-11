//! The catalog a client keeps on its device, and the SQL shared by the server and that device
//! (`design/general.md` §3). This holds the device schema; the server's handlers and the web
//! app's catalog worker (`crates/catalog-wasm`) both run what is built on it.

use rusqlite::Connection;

pub mod sync;

/// The device schema's version. Raise it whenever [`SCHEMA`] changes: a device whose database
/// carries another version deletes it and syncs again, since it is only a copy of the server's.
pub const SCHEMA_VERSION: i32 = 1;

/// The device schema's DDL: a subset of the server's catalog tables with the same table and
/// column names and types, minus what never leaves the server.
pub const SCHEMA: &str = include_str!("schema.sql");

/// Creates the device schema in an empty database and stamps it with [`SCHEMA_VERSION`] as its
/// `user_version`.
pub fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_creates_and_is_stamped() {
        let conn = Connection::open_in_memory().unwrap();
        create_schema(&conn).unwrap();
        let tables: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_schema WHERE type = 'table'", [], |r| r.get(0))
            .unwrap();
        let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!((tables, version), (13, SCHEMA_VERSION));
    }
}
