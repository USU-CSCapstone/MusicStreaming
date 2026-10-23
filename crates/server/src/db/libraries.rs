//! Library rows and their roots, as the scanner needs them, and where a library's files are.

use std::path::{Path, PathBuf};

use jewelcase_scanner::{FileToDecode, LibraryConfig};
use rusqlite::{Connection, Result, params};

use super::{Json, new_id, now_ms};

/// One of a library's root folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub id: i64,
    pub path: PathBuf,
}

/// A library's settings and active roots.
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
    /// What the scanner needs to know about this library.
    pub fn config(&self) -> LibraryConfig {
        LibraryConfig {
            id: self.id,
            roots: self.roots.iter().map(|r| r.path.clone()).collect(),
            excludes: self.excludes.clone(),
        }
    }
}

/// Every library with its active roots, oldest first. Exclude patterns that cannot be read are
/// an error.
pub fn all(conn: &Connection) -> Result<Vec<LibraryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, excludes, watch, scan_interval_minutes FROM libraries ORDER BY created_at, id",
    )?;
    // Unreadable patterns fail loudly: read as none, they would scan what the admin excluded.
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Json<Vec<String>>>(2)?.0,
            r.get::<_, i64>(3)? != 0,
            r.get::<_, Option<i64>>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, name, excludes, watch, interval) = row?;
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

/// The library's active roots: every path into the library is resolved against these.
pub fn roots(conn: &Connection, library_id: i64) -> Result<Vec<Root>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, path FROM library_roots WHERE library_id = ?1 AND removed_at IS NULL ORDER BY path",
    )?;
    let rows = stmt.query_map([library_id], |r| {
        Ok(Root { id: r.get(0)?, path: PathBuf::from(r.get::<_, String>(1)?) })
    })?;
    rows.collect()
}

/// Whether a scan of the library has completed. Until one has, its next library scan is its
/// first.
pub fn scanned(conn: &Connection, library_id: i64) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM scans WHERE library_id = ?1 AND state = 'completed')",
        [library_id],
        |r| r.get(0),
    )
}

/// The track's or image's file at `rel` under root `root_id` of `library`, where its row places
/// it, or `None` when that root is not one of the library's active roots. A symlink that leads
/// there is refused before the file is opened (`requirements/scanning.md` §5).
pub fn file(
    conn: &Connection,
    library: i64,
    root_id: i64,
    rel: &str,
) -> Result<Option<FileToDecode>> {
    let root = roots(conn, library)?.into_iter().find(|r| r.id == root_id);
    Ok(root.map(|root| FileToDecode { path: root.path.join(rel), root: root.path }))
}

/// Creates a library with its roots, and returns its id: `id` if given (for tests and imports),
/// otherwise a new one. Roots are stored as given; callers should pass canonical absolute
/// paths.
pub fn create(
    conn: &Connection,
    id: Option<i64>,
    name: &str,
    roots: &[&Path],
    excludes: &[String],
) -> Result<i64> {
    let id = match id {
        Some(id) => id,
        None => new_id()?,
    };
    conn.execute(
        "INSERT INTO libraries (id, name, excludes, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![id, name, Json(excludes), now_ms()],
    )?;
    for root in roots {
        add_root(conn, id, root)?;
    }
    Ok(id)
}

/// Adds a root to the library and returns its id. When an active root of any library is
/// already at `path`, nothing is added and that root's id is returned.
pub fn add_root(conn: &Connection, library_id: i64, path: &Path) -> Result<i64> {
    conn.query_row(
        "INSERT INTO library_roots (id, library_id, path, created_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (path) WHERE removed_at IS NULL DO UPDATE SET path = excluded.path RETURNING id",
        params![new_id()?, library_id, path.to_string_lossy(), now_ms()],
        |r| r.get(0),
    )
}

/// Whether `a` and `b` are the same directory, or one is inside the other. It compares whole
/// components and reads nothing from disk, so both should be canonical.
pub fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library created without an id is given a drawn one, and that is its row's id.
    #[test]
    fn a_library_created_without_an_id_is_given_a_drawn_one() {
        let (_temp, db) = crate::db::testing::open();
        let (id, stored) = db
            .write_blocking(|tx| {
                let id = create(tx, None, "Music", &[Path::new("/music")], &[])?;
                Ok((id, tx.query_row("SELECT id FROM libraries", [], |r| r.get::<_, i64>(0))?))
            })
            .unwrap();
        assert_eq!(stored, id);
        assert!(id > i64::from(u32::MAX), "a drawn id, not a rowid");
    }

    #[test]
    fn directories_overlap_when_one_holds_the_other() {
        let overlap = |a: &str, b: &str| overlap(Path::new(a), Path::new(b));
        assert!(overlap("/srv/music", "/srv/music"));
        assert!(overlap("/srv/music", "/srv/music/.jewelcase"));
        assert!(overlap("/srv/music/.jewelcase", "/srv/music"));
        assert!(!overlap("/srv/music", "/srv/jewelcase"));
        assert!(!overlap("/srv/music", "/srv/music2"));
    }
}
