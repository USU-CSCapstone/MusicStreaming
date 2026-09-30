//! Library rows and their roots, as the scanner needs them.

use std::path::{Path, PathBuf};

use jewelcase_scanner::LibraryConfig;
use rusqlite::Error::FromSqlConversionFailure;
use rusqlite::types::Type;
use rusqlite::{Connection, Result, params};

use super::now_ms;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub id: i64,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LibraryRow {
    pub id: i64,
    pub name: String,
    pub excludes: Vec<String>,
    pub watch: bool,
    pub scan_interval_minutes: Option<i64>,
    pub roots: Vec<Root>,
}

impl LibraryRow {
    pub fn config(&self) -> LibraryConfig {
        LibraryConfig {
            id: self.id.to_string(),
            roots: self.roots.iter().map(|r| r.path.clone()).collect(),
            excludes: self.excludes.clone(),
        }
    }
}

pub fn all(conn: &Connection) -> Result<Vec<LibraryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, excludes, watch, scan_interval_minutes FROM libraries ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)? != 0,
            r.get::<_, Option<i64>>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, name, excludes, watch, interval) = row?;
        // Unreadable patterns fail loudly: read as none, they would scan what the admin excluded.
        let excludes: Vec<String> = serde_json::from_str(&excludes)
            .map_err(|error| FromSqlConversionFailure(2, Type::Text, Box::new(error)))?;
        out.push(LibraryRow {
            id,
            name,
            excludes,
            watch,
            scan_interval_minutes: interval,
            roots: roots(conn, id)?,
        });
    }
    Ok(out)
}

pub fn roots(conn: &Connection, library_id: i64) -> Result<Vec<Root>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, path FROM library_roots WHERE library_id = ?1 AND removed_at IS NULL ORDER BY path",
    )?;
    let rows = stmt.query_map([library_id], |r| {
        Ok(Root {
            id: r.get(0)?,
            path: PathBuf::from(r.get::<_, String>(1)?),
        })
    })?;
    rows.collect()
}

/// Create a library with its roots, and return its id: `id` if given (for tests and imports),
/// otherwise a random one. Roots are stored as given; callers should pass canonical absolute
/// paths.
pub fn create(
    conn: &Connection,
    id: Option<i64>,
    name: &str,
    roots: &[&Path],
    excludes: &[String],
) -> Result<i64> {
    let id = conn.query_row(
        "INSERT INTO libraries (id, name, excludes, created_at, updated_at) \
         VALUES (coalesce(?1, random() & 0x7FFFFFFFFFFFFFFF), ?2, ?3, ?4, ?4) RETURNING id",
        params![id, name, serde_json::to_string(excludes).unwrap(), now_ms()],
        |r| r.get(0),
    )?;
    for root in roots {
        add_root(conn, id, root)?;
    }
    Ok(id)
}

/// Add a root and return its id, or the id of the active root already at `path`.
pub fn add_root(conn: &Connection, library_id: i64, path: &Path) -> Result<i64> {
    conn.query_row(
        "INSERT INTO library_roots (id, library_id, path, created_at) VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3) \
         ON CONFLICT (path) WHERE removed_at IS NULL DO UPDATE SET path = excluded.path RETURNING id",
        params![library_id, path.to_string_lossy(), now_ms()],
        |r| r.get(0),
    )
}
