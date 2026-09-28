//! Schema migrations, compiled into the binary and applied at startup
//! (requirements/deployment.md §5).
//!
//! `PRAGMA user_version` records how many have been applied. Each migration runs in one
//! transaction with its version bump, so an interrupted upgrade leaves the previous version
//! intact and the next start picks up where it stopped.

use anyhow::{Context, bail};
use rusqlite::{Connection, TransactionBehavior};
use tracing::info;

pub struct Migration {
    pub name: &'static str,
    pub sql: &'static str,
}

/// Every file in `migrations/`, in order, listed by build.rs
pub const MIGRATIONS: &[Migration] = include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// Applies every migration the database has not seen yet, and returns the schema version.
pub fn migrate(conn: &mut Connection, migrations: &[Migration]) -> anyhow::Result<usize> {
    let applied = version(conn)?;
    if applied > migrations.len() {
        bail!(
            "the database is at schema version {applied}, but this version of Jewelcase only \
             knows {}; run the newer version, or restore a backup taken before upgrading",
            migrations.len()
        );
    }

    for (index, migration) in migrations.iter().enumerate().skip(applied) {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(migration.sql)
            .with_context(|| format!("migration {} failed", migration.name))?;
        tx.pragma_update(None, "user_version", (index + 1) as u32)?;
        tx.commit()?;
        info!(migration = migration.name, "applied migration");
    }

    Ok(migrations.len())
}

fn version(conn: &Connection) -> rusqlite::Result<usize> {
    let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(version as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    }

    #[test]
    fn applies_everything_to_a_new_database_once() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(migrate(&mut conn, MIGRATIONS).unwrap(), MIGRATIONS.len());
        assert_eq!(version(&conn).unwrap(), MIGRATIONS.len());
        assert!(table_exists(&conn, "tracks"));

        // A second run finds nothing to do.
        assert_eq!(migrate(&mut conn, MIGRATIONS).unwrap(), MIGRATIONS.len());
    }

    #[test]
    fn a_failed_migration_leaves_the_previous_version_intact() {
        let migrations = [
            Migration {
                name: "0001_good",
                sql: "CREATE TABLE a (x INTEGER);",
            },
            Migration {
                name: "0002_bad",
                sql: "CREATE TABLE b (x INTEGER); SELECT * FROM missing;",
            },
        ];
        let mut conn = Connection::open_in_memory().unwrap();
        let error = migrate(&mut conn, &migrations).unwrap_err();
        assert!(format!("{error:#}").contains("0002_bad"), "{error:#}");
        assert_eq!(version(&conn).unwrap(), 1);
        assert!(table_exists(&conn, "a"));
        assert!(!table_exists(&conn, "b"));
    }

    #[test]
    fn refuses_a_database_from_a_newer_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        let error = migrate(&mut conn, MIGRATIONS).unwrap_err();
        assert!(error.to_string().contains("schema version 99"), "{error:#}");
    }
}
