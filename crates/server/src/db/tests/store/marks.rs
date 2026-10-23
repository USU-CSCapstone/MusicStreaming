//! Marking tracks missing: each track is marked, counted and fed once, however often the mark is
//! applied, and a folder gone from its parent's listing is marked a batch at a time.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jewelcase_scanner::testing::{self, Hooked, InspectStore};
use jewelcase_scanner::*;

use super::{count, make, record, scalar};
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

/// A track gone from its folder's listing is marked once.
#[test]
fn a_missing_track_is_marked_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, store) = scanned(tmp.path());
    let id: TrackId = scalar(&store, "SELECT id FROM tracks ORDER BY path LIMIT 1").unwrap();
    marks_once(&store, || Batch { missing: vec![id], ..Batch::default() }, 1);
}

/// A folder of 5,000 tracks gone from its parent's listing, and 1,000 files gone from a folder
/// still there, are marked a batch at a time, never more in one apply than a batch holds at
/// most, and every one of their tracks ends missing. A rescan puts none of them in a batch again.
#[test]
fn missing_tracks_are_marked_a_batch_at_a_time() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    let store = Arc::new(make(&root));
    let (gone, kept) = (root.join("Gone"), root.join("A/X"));
    let records: Vec<TrackRecord> = (0..6_000)
        .map(|i| {
            let record = record(&root, i);
            let path = match i {
                0..5_000 => gone.join(record.path.strip_prefix(&root).unwrap()),
                _ => kept.join(format!("{i}.flac")),
            };
            TrackRecord { path, ..record }
        })
        .collect();
    for upserts in records.chunks(250) {
        store.apply(1, Batch { upserts: upserts.to_vec(), ..Batch::default() }).unwrap();
    }
    let missing = || count(&store, "SELECT count(*) FROM tracks WHERE missing_since IS NOT NULL");
    let before_each = Mutex::new(Vec::new());
    let hooked = Hooked::new(store.clone()).on_apply(|_| {
        before_each.lock().unwrap().push(missing());
        Ok(())
    });

    let scan = testing::scan_library(&hooked, &testing::library(&root));
    assert_eq!((scan.state, scan.progress.missing), (ScanState::Completed, 6_000));
    let mut counts = before_each.lock().unwrap().clone();
    counts.push(missing());
    let most = counts.windows(2).map(|pair| pair[1] - pair[0]).max().unwrap();
    let batch_size = i64::try_from(ScanOptions::default().batch_size).unwrap();
    assert!(most <= batch_size, "one apply marked {most}");
    assert_eq!(counts.last(), Some(&6_000));

    let sent_again = Mutex::new(0);
    let rescanning = Hooked::new(store.clone()).on_apply(|batch| {
        *sent_again.lock().unwrap() += batch.missing.len();
        Ok(())
    });
    testing::scan_library(&rescanning, &testing::library(&root));
    assert_eq!(*sent_again.lock().unwrap(), 0);
}
