//! Library rows and their roots, as the scanner needs them, and where a library's files are.

use std::io;
use std::path::{Path, PathBuf};

use jewelcase_scanner::{FileToDecode, LibraryConfig, without_links};
use rusqlite::{Connection, Result, params};

use super::{Json, new_id, now_ms};

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

/// A track's or image's file where its row places it: under one of its library's active roots,
/// and refused before it is opened if a symlink leads there (`requirements/scanning.md` §5).
#[derive(Clone)]
pub struct LibraryFile {
    root: PathBuf,
    path: PathBuf,
}

impl LibraryFile {
    /// The file's real path. `NotFound` when it is missing or reached through a symlink. Touches
    /// the filesystem, so call it off the async threads and the database's.
    pub fn resolve(&self) -> io::Result<PathBuf> {
        without_links(&self.root, &self.path)
    }

    /// The file's real path, once it opens, refused as [`FileToDecode::open`] refuses it: an
    /// error when it is missing, reached through a symlink, not a regular file, or may not be
    /// read.
    /// Touches the filesystem, so call it off the async threads and the database's.
    pub fn open(self) -> io::Result<PathBuf> {
        FileToDecode { root: self.root, path: self.path }.open()
    }
}

/// The file at `rel` under root `root_id` of `library`, or `None` when that root is not one of
/// the library's active roots.
pub fn file(
    conn: &Connection,
    library: i64,
    root_id: i64,
    rel: &str,
) -> Result<Option<LibraryFile>> {
    let root = roots(conn, library)?.into_iter().find(|r| r.id == root_id);
    Ok(root.map(|root| LibraryFile { path: root.path.join(rel), root: root.path }))
}

/// Create a library with its roots, and return its id: `id` if given (for tests and imports),
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

/// Add a root and return its id, or the id of the active root already at `path`.
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
    use crate::db::Database;

    /// A library created without an id is given a drawn one, and that is its row's id.
    #[test]
    fn a_library_created_without_an_id_is_given_a_drawn_one() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(&temp.path().join("jewelcase.db")).unwrap();
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
