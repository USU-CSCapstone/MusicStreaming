//! A completed scan marks each root it walked whole as walked when it started, and no other root,
//! so the sweep purges only what such a walk found still missing (`design/database.md` §9).

use std::fs;
use std::path::Path;

use jewelcase_scanner::testing::{self, ItemKey};
use jewelcase_scanner::*;

use super::{count, library_store, tagged};
use crate::db::retention::WINDOW_MS;
use crate::db::testing::sweep;
use crate::db::{libraries, now_ms};

/// The `walked_at` of root `root`.
fn walked_at(store: &crate::db::store::SqliteStore, root: &Path) -> Option<i64> {
    let root = root.to_string_lossy().into_owned();
    let sql = "SELECT walked_at FROM library_roots WHERE path = ?1";
    store.db().read_blocking(move |conn| conn.query_row(sql, [root], |r| r.get(0))).unwrap()
}

/// Roots a and b of library 1 each hold a track missing past the window, marked by a scan of
/// both before the window. A completed scan of a alone purges a's track and keeps b's, and leaves
/// library 2's root unwalked.
#[test]
fn a_scan_confirms_only_the_roots_it_walked_whole() {
    let tmp = tempfile::tempdir().unwrap();
    let [a, b, other] = ["a", "b", "other"].map(|name| tmp.path().join(name));
    for root in [&a, &b] {
        for track in ["1", "2"] {
            let tags = [(ItemKey::TrackTitle, track), (ItemKey::AlbumTitle, "X")];
            tagged(root, &format!("X/{track}.wav"), &tags);
        }
    }
    let lib = LibraryConfig { roots: vec![a.clone(), b.clone()], ..testing::library(&a) };
    let store = library_store(&lib);
    let made = other.clone();
    store
        .db()
        .write_blocking(move |tx| libraries::create(tx, Some(2), "Other", &[&made], &[]).map(drop))
        .unwrap();
    store.refresh_roots().unwrap();
    testing::scan_library(&store, &lib);
    fs::remove_file(a.join("X/1.wav")).unwrap();
    fs::remove_file(b.join("X/1.wav")).unwrap();
    testing::scan_library(&store, &lib);
    let ms = WINDOW_MS + 60_000;
    let aged = move |tx: &rusqlite::Transaction<'_>| {
        tx.execute("UPDATE tracks SET missing_since = missing_since - ?1", [ms])?;
        tx.execute("UPDATE library_roots SET walked_at = walked_at - ?1", [ms])
    };
    store.db().write_blocking(aged).unwrap();

    testing::scan_once(&store, &lib, Trigger::Manual, vec![Scope::root(&a)]);
    sweep(store.db(), now_ms());
    let missing = "SELECT count(*) FROM tracks t JOIN library_roots r ON r.id = t.root_id \
                   WHERE t.missing_since IS NOT NULL AND r.path LIKE '%/b'";
    assert_eq!(count(&store, "SELECT count(*) FROM tracks"), 3);
    assert_eq!(count(&store, missing), 1, "b's track stays");
    assert_eq!(walked_at(&store, &other), None);
}

/// A completed scan's walk dates from when it started, not when it finished, since a file it
/// found missing early on may have come back before it ended.
#[test]
fn a_walk_dates_from_the_start_of_its_scan() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let lib = testing::library(&root);
    let store = library_store(&lib);
    let mut scan = store.create_scan(lib.id, Trigger::Manual, &[Scope::root(&root)]).unwrap();
    let started = 1_000_000;
    scan.state = ScanState::Completed;
    (scan.started_at, scan.finished_at) = (Some(started), Some(started + 3_600_000));
    store.update_scan(&scan).unwrap();
    assert_eq!(walked_at(&store, &root), Some(started));
}
