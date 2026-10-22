//! Marking tracks missing, by ID and by a folder gone from its parent's listing: each track is
//! marked, counted and fed once, however often the mark is applied.

use std::path::{Path, PathBuf};

use jewelcase_scanner::testing::{self, InspectStore};
use jewelcase_scanner::*;

use super::{make, scalar};
use crate::db::store::SqliteStore;

/// A store whose library 1 holds A/X's two tracks and B/Y's one, scanned under a root in `tmp`,
/// and that root.
fn scanned(tmp: &Path) -> (PathBuf, SqliteStore) {
    let root = tmp.join("lib");
    testing::make_album(&root, "A", "X", 2);
    testing::make_album(&root, "B", "Y", 1);
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    (root, store)
}

/// When each track went missing, in path order.
fn marks(store: &SqliteStore) -> Vec<Option<i64>> {
    let sql = "SELECT missing_since FROM tracks ORDER BY path";
    let read =
        |conn: &rusqlite::Connection| conn.prepare(sql)?.query_map([], |r| r.get(0))?.collect();
    store.db().read_blocking(read).unwrap()
}

/// `batch` marks `n` tracks missing and feeds them, then marks, counts and feeds nothing when it
/// is applied again, leaving each track's time.
fn marks_once(store: &SqliteStore, batch: impl Fn() -> Batch, n: i64) {
    let head = store.feed_head(1);
    assert_eq!(store.apply(1, batch()).unwrap(), n);
    let (marked, fed) = (marks(store), store.feed_head(1));
    assert_eq!(marked.iter().flatten().count(), usize::try_from(n).unwrap(), "{marked:?}");
    assert!(fed > head);
    assert_eq!(store.apply(1, batch()).unwrap(), 0);
    assert_eq!((marks(store), store.feed_head(1)), (marked, fed));
}

/// A folder gone from its parent's listing marks every track under it once.
#[test]
fn a_vanished_folder_is_marked_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, store) = scanned(tmp.path());
    marks_once(&store, || Batch { vanished: vec![root.join("A")], ..Batch::default() }, 2);
}

/// A track gone from its folder's listing is marked once.
#[test]
fn a_missing_track_is_marked_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, store) = scanned(tmp.path());
    let id: TrackId = scalar(&store, "SELECT id FROM tracks ORDER BY path LIMIT 1").unwrap();
    marks_once(&store, || Batch { missing: vec![id], ..Batch::default() }, 1);
}
