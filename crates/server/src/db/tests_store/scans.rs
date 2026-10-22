//! How a scan row keeps its paths: each scope's root by id and every path relative to its root,
//! so no storage path is stored and a scan resumes under wherever its roots now are.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use jewelcase_scanner::testing;
use jewelcase_scanner::*;
use serde_json::{Value, json};

use super::{count, library_store, make, scalar};
use crate::db::libraries;
use crate::db::store::SqliteStore;

/// A store whose library 1 has its one root at `root`, and a running scan of that root and of
/// its folder `a`, saved with its cursor after `a` and its current path at `a/b`.
fn saved_scan(root: &Path) -> (SqliteStore, Scan) {
    fs::create_dir_all(root.join("a/b")).unwrap();
    let store = make(root);
    let scopes = [Scope::root(root), Scope::folder(root, root.join("a"), Depth::Directory)];
    let mut scan = store.create_scan(1, Trigger::Manual, &scopes).unwrap();
    scan.state = ScanState::Running;
    scan.cursor = Some(Cursor { scope_index: 1, after_directory: Some(root.join("a")) });
    scan.progress.current_path = Some(root.join("a/b"));
    store.update_scan(&scan).unwrap();
    (store, scan)
}

/// A save of a scan row names its library, so a scan that names another library changes no row.
#[test]
fn a_scan_row_is_saved_only_in_its_library() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, scan) = saved_scan(&tmp.path().join("lib"));
    let elsewhere = Scan { library: 2, state: ScanState::Completed, ..scan.clone() };
    store.update_scan(&elsewhere).unwrap();
    assert_eq!(store.scans(1).unwrap(), [scan]);
}

/// Adds library 2 to `store`, with its one root at `other`, and returns that root's id.
fn other_library(store: &SqliteStore, other: &Path) -> i64 {
    let other = other.to_path_buf();
    let create =
        move |tx: &rusqlite::Transaction| libraries::create(tx, Some(2), "Other", &[&other], &[]);
    store.db().write_blocking(create).unwrap();
    store.refresh_roots().unwrap();
    scalar(store, "SELECT id FROM library_roots WHERE library_id = 2").unwrap()
}

/// The `cursor_dir` of the one scan in `store`.
fn cursor_dir(store: &SqliteStore) -> Option<String> {
    scalar(store, "SELECT cursor_dir FROM scans")
}

/// `scan` with every path under `from` moved under `to`.
fn moved(scan: &Scan, from: &Path, to: &Path) -> Scan {
    let mv = |path: &Path| to.join(path.strip_prefix(from).unwrap());
    let mut scan = scan.clone();
    for scope in &mut scan.scopes {
        (scope.root, scope.path) = (mv(&scope.root), mv(&scope.path));
    }
    let cursor = scan.cursor.as_mut().unwrap();
    cursor.after_directory = cursor.after_directory.as_deref().map(mv);
    scan.progress.current_path = scan.progress.current_path.as_deref().map(mv);
    scan
}

#[test]
fn a_scan_row_keeps_its_paths_relative_to_their_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, scan) = saved_scan(&tmp.path().join("lib"));
    let root: i64 = scalar(&store, "SELECT id FROM library_roots").unwrap();
    let row = store
        .db()
        .read_blocking(|conn| {
            conn.query_row(
                "SELECT scopes, cursor_dir, current_root_id, current_path FROM scans",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
        })
        .unwrap();
    let (scopes, cursor_dir, current_root, current_path): (_, String, i64, String) = row;
    assert_eq!(
        serde_json::from_str::<Value>(&scopes).unwrap(),
        json!([
            { "root": root, "path": "", "depth": "subtree" },
            { "root": root, "path": "a", "depth": "directory" },
        ])
    );
    assert_eq!((cursor_dir.as_str(), current_root, current_path.as_str()), ("a", root, "a/b"));
    assert_eq!(store.scans(1).unwrap(), [scan], "read back as it was saved");
}

/// A root repointed while a scan was interrupted takes the scan with it.
#[test]
fn a_scan_resumes_under_its_repointed_root() {
    let tmp = tempfile::tempdir().unwrap();
    let (old, new) = (tmp.path().join("lib"), tmp.path().join("moved"));
    let (store, scan) = saved_scan(&old);
    let path = new.to_str().unwrap().to_owned();
    let repoint =
        move |tx: &rusqlite::Transaction| tx.execute("UPDATE library_roots SET path = ?1", [path]);
    store.db().write_blocking(repoint).unwrap();
    store.refresh_roots().unwrap();
    assert_eq!(store.interrupted_scans(1).unwrap(), [moved(&scan, &old, &new)]);
}

/// A scan whose root has been removed has nothing left to resume, so its row is suspended.
#[test]
fn a_scan_of_a_removed_root_is_suspended() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, _) = saved_scan(&tmp.path().join("lib"));
    store
        .db()
        .write_blocking(|tx| tx.execute_batch("UPDATE library_roots SET removed_at = 1"))
        .unwrap();
    store.refresh_roots().unwrap();
    assert!(store.interrupted_scans(1).unwrap().is_empty());
    assert_eq!(count(&store, "SELECT COUNT(*) FROM scans WHERE state = 'suspended'"), 1);
}

/// A scope outside the library's roots, even in another library's, has no root of the library to
/// keep it by, so no scan of it is recorded there.
#[test]
fn a_scan_outside_the_library_is_not_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, other) = (tmp.path().join("lib"), tmp.path().join("other"));
    fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    other_library(&store, &other);
    for scope in [Scope::root(&other), Scope::folder(&root, &other, Depth::Subtree)] {
        assert!(store.create_scan(1, Trigger::Manual, &[scope]).is_err());
    }
    assert!(store.scans(1).unwrap().is_empty());
    store.create_scan(2, Trigger::Manual, &[Scope::root(&other)]).unwrap();
}

/// A scope that names another library's root is never resumed in this one.
#[test]
fn a_scope_naming_another_librarys_root_is_not_resumed() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, _) = saved_scan(&tmp.path().join("lib"));
    let other = other_library(&store, &tmp.path().join("other"));
    // No cursor, so only the scope's root can make the row unreadable.
    let scopes = json!([{ "root": other, "path": "", "depth": "subtree" }]).to_string();
    let sql = "UPDATE scans SET scopes = ?1, cursor_scope = NULL, cursor_dir = NULL";
    store.db().write_blocking(move |tx| tx.execute(sql, [scopes])).unwrap();
    assert!(store.interrupted_scans(1).unwrap().is_empty());
    assert_eq!(count(&store, "SELECT COUNT(*) FROM scans WHERE state = 'suspended'"), 1);
}

/// The folder being scanned is always in a root of the scan's own library.
#[test]
fn the_current_folder_cannot_be_in_another_librarys_root() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, _) = saved_scan(&tmp.path().join("lib"));
    let other = other_library(&store, &tmp.path().join("other"));
    let sql = "UPDATE scans SET current_root_id = ?1";
    assert!(store.db().write_blocking(move |tx| tx.execute(sql, [other])).is_err());
}

/// The cursor's folder is kept relative to the root of its own scope, not the first scope's.
#[test]
fn the_cursor_is_kept_relative_to_its_scopes_root() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(b.join("X")).unwrap();
    let store =
        library_store(&LibraryConfig { roots: vec![a.clone(), b.clone()], ..testing::library(&a) });
    let mut scan =
        store.create_scan(1, Trigger::Manual, &[Scope::root(&a), Scope::root(&b)]).unwrap();
    scan.cursor = Some(Cursor { scope_index: 1, after_directory: Some(b.join("X")) });
    store.update_scan(&scan).unwrap();
    assert_eq!(cursor_dir(&store).as_deref(), Some("X"));
    assert_eq!(store.scans(1).unwrap(), [scan]);
}

/// A cursor folder whose path is not UTF-8 is kept as its nearest ancestor that is, which the
/// resume walks again, rather than as a lossy spelling that could sort past unscanned folders.
#[test]
fn a_cursor_folder_that_is_not_text_is_kept_as_its_nearest_text_ancestor() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    let mut scan = store.create_scan(1, Trigger::Manual, &[Scope::root(&root)]).unwrap();
    let folder = root.join("a").join(OsStr::from_bytes(b"caf\xe9")).join("b");
    scan.cursor = Some(Cursor { scope_index: 0, after_directory: Some(folder) });
    store.update_scan(&scan).unwrap();
    assert_eq!(cursor_dir(&store).as_deref(), Some("a"));
    let resumed = Cursor { scope_index: 0, after_directory: Some(root.join("a")) };
    assert_eq!(store.scans(1).unwrap()[0].cursor, Some(resumed));
}
