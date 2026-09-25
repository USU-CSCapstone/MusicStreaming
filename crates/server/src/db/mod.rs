//! The SQLite database (`design/database.md`).
//!
//! One writer connection for the whole process behind a mutex, since SQLite
//! has one writer; WAL so readers never wait on a batch commit; fresh
//! read-only connections for request handlers. Every connection turns
//! foreign keys on, because the schema's isolation depends on them.

// Read-side helpers here are used by the admin API as it is built and by
// the tests meanwhile.
#![allow(dead_code)]

pub mod catalog;
pub mod feed;
pub mod libraries;
pub mod store;
#[cfg(test)]
mod tests_store;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::Connection;

pub use store::SqliteStore;

/// Migrations, applied in order. Plain SQL files in `crates/server/migrations/`.
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_libraries",
        include_str!("../../migrations/0001_libraries.sql"),
    ),
    (
        "0002_accounts",
        include_str!("../../migrations/0002_accounts.sql"),
    ),
    (
        "0003_catalog",
        include_str!("../../migrations/0003_catalog.sql"),
    ),
    (
        "0004_feeds",
        include_str!("../../migrations/0004_feeds.sql"),
    ),
];

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("database schema is newer ({found}) than this server knows ({known})")]
    SchemaTooNew { found: i32, known: i32 },
}

pub type Result<T> = std::result::Result<T, DbError>;

pub struct Db {
    path: PathBuf,
    writer: Mutex<Connection>,
}

impl Db {
    /// Open or create the database at `path`, configure it, and migrate.
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        configure(&conn)?;
        migrate(&conn)?;
        Ok(Db {
            path: path.to_path_buf(),
            writer: Mutex::new(conn),
        })
    }

    /// The single writer. Hold it only for the length of one transaction.
    pub fn writer(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A fresh read-only connection for a request handler.
    pub fn reader(&self) -> Result<Connection> {
        let conn = Connection::open(&self.path)?;
        configure(&conn)?;
        conn.pragma_update(None, "query_only", true)?;
        Ok(conn)
    }

    /// Give the planner statistics (`design/database.md` §4). Cheap; run
    /// periodically on the writer.
    pub fn optimize(&self) -> Result<()> {
        self.writer().execute_batch("PRAGMA optimize;")?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i32> {
        Ok(self
            .writer()
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }
}

fn configure(conn: &Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(())
}

fn migrate(conn: &Connection) -> Result<()> {
    let current: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let known = MIGRATIONS.len() as i32;
    if current > known {
        return Err(DbError::SchemaTooNew {
            found: current,
            known,
        });
    }
    for (i, (name, sql)) in MIGRATIONS.iter().enumerate() {
        let version = i as i32 + 1;
        if version <= current {
            continue;
        }
        tracing::info!(migration = name, "applying");
        conn.execute_batch("BEGIN;")?;
        if let Err(e) = conn.execute_batch(sql) {
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(e.into());
        }
        conn.pragma_update(None, "user_version", version)?;
        conn.execute_batch("COMMIT;")?;
    }
    Ok(())
}

/// A random positive 63-bit id (`design/database.md` §1). Callers retry on
/// a unique-constraint collision.
pub fn new_id() -> i64 {
    loop {
        let v = rand::random::<u64>() & 0x7FFF_FFFF_FFFF_FFFF;
        if v != 0 {
            return v as i64;
        }
    }
}

pub fn now_ms() -> i64 {
    jewelcase_scanner::now_ms() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_migrates_and_reopens() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state/jewelcase.db");
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as i32);
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as i32);
        let fk: bool = db
            .reader()
            .unwrap()
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .unwrap();
        assert!(fk);
    }

    #[test]
    fn ids_are_positive_and_distinct() {
        let a = new_id();
        let b = new_id();
        assert!(a > 0 && b > 0);
        assert_ne!(a, b);
    }
}
