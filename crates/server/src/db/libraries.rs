//! Library rows and their roots, as the scanner needs them.

use std::path::{Path, PathBuf};

use jewelcase_scanner::LibraryConfig;
use rusqlite::{Connection, OptionalExtension, params};

use super::{Result, new_id, now_ms};

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
        let excludes: Vec<String> = serde_json::from_str(&excludes).unwrap_or_default();
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
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Create a library with its roots. Roots are stored as given; callers
/// should pass canonical absolute paths.
pub fn create(conn: &Connection, name: &str, roots: &[&Path], excludes: &[String]) -> Result<i64> {
    let now = now_ms();
    let id = loop {
        let id = new_id();
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO libraries (id, name, excludes, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
                params![id, name, serde_json::to_string(excludes).unwrap(), now],
            )?;
        if inserted == 1 {
            break id;
        }
    };
    for root in roots {
        add_root(conn, id, root)?;
    }
    Ok(id)
}

/// Create a library with a caller-chosen id. For tests and imports.
pub fn create_with_id(
    conn: &Connection,
    id: i64,
    name: &str,
    roots: &[&Path],
    excludes: &[String],
) -> Result<()> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO libraries (id, name, excludes, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![id, name, serde_json::to_string(excludes).unwrap(), now],
    )?;
    for root in roots {
        add_root(conn, id, root)?;
    }
    Ok(())
}

pub fn add_root(conn: &Connection, library_id: i64, path: &Path) -> Result<i64> {
    let now = now_ms();
    loop {
        let id = new_id();
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO library_roots (id, library_id, path, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, library_id, path.to_string_lossy(), now],
        )?;
        if inserted == 1 {
            return Ok(id);
        }
        // Either an id collision (retry) or the path is already a root.
        if let Some(existing) = conn
            .query_row(
                "SELECT id FROM library_roots WHERE path = ?1 AND removed_at IS NULL",
                [path.to_string_lossy()],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        {
            return Ok(existing);
        }
    }
}
