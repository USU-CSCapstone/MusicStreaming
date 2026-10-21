//! A scan stopped partway through a folder, which its cursor has not passed, resumes with every
//! file indexed and counted once (`design/scanning.md` §9, §13).

use std::os::unix::fs::symlink;

use super::*;

/// A/X with one track, then B/Y with a file that is not audio, four tracks, and a symlink, whose
/// tracks and problems take several batches.
fn two_folders(root: &Path) {
    make_album(root, "A", "X", 1);
    make_album(root, "B", "Y", 4);
    fs::write(root.join("B/Y/00 - Broken.flac"), b"not audio").unwrap();
    symlink(track(root, "A/X", 1), root.join("B/Y/zz - Link.wav")).unwrap();
}

/// A shutdown asked for as B/Y's third track is read leaves the row running at B, the last
/// folder completed, counting only what a resume will not count again: A/X's track seen, and the
/// four tracks stored. The resumed scan ends with the counts and tracks of one never stopped.
#[test]
fn a_scan_shut_down_partway_through_a_folder_resumes_counting_each_file_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    two_folders(&root);
    let lib = library(&root);
    let stops = Stops::default();
    let stop_at = track(&root, "B/Y", 3);
    let on_lookup = |path: &Path| {
        if path == stop_at {
            stops.shutdown.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    let store =
        Hooked { inner: MemoryStore::new(), on_apply: || Ok(()), on_lookup, on_under: || Ok(()) };
    let mut scan = new_scan(&store, &lib);
    run(&store, &lib, &mut scan, &batches_of(2), &stops);

    let mut row = only_row(&store, &lib);
    let stopped = ScanProgress {
        files_seen: 1,
        files_processed: 4,
        added: 4,
        current_path: Some(root.join("B/Y")),
        ..Default::default()
    };
    let cursor = after(&root, "B");
    assert_eq!((row.state, &row.cursor, &row.progress), (ScanState::Running, &cursor, &stopped));
    assert_eq!(store.inner.tracks(lib.id).len(), 4);

    run(&store, &lib, &mut row, &batches_of(2), &Stops::default());
    assert_resumed_like_an_uninterrupted_scan(&store, &lib);
}

/// A batch that fails partway through B/Y suspends the scan at B, the last folder completed, with
/// the progress the store holds counting exactly the tracks it stored. Run again, the scan ends
/// with the counts and tracks of one never stopped.
#[test]
fn a_scan_suspended_partway_through_a_folder_resumes_counting_each_file_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    two_folders(&root);
    let lib = library(&root);
    // The first batch applied after B/Y's third track is first read fails, and no other.
    let (armed, fired) = (AtomicBool::new(false), AtomicBool::new(false));
    let arm_at = track(&root, "B/Y", 3);
    let on_lookup = |path: &Path| {
        if path == arm_at && !fired.load(Ordering::SeqCst) {
            armed.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    let on_apply = || {
        if armed.swap(false, Ordering::SeqCst) {
            fired.store(true, Ordering::SeqCst);
            Err(StoreError("disk full".into()))
        } else {
            Ok(())
        }
    };
    let store = Hooked { inner: MemoryStore::new(), on_apply, on_lookup, on_under: || Ok(()) };
    let mut scan = new_scan(&store, &lib);
    run(&store, &lib, &mut scan, &batches_of(2), &Stops::default());

    let mut row = only_row(&store, &lib);
    assert_eq!((row.state, &row.cursor), (ScanState::Suspended, &after(&root, "B")), "{row:?}");
    let stored = i64::try_from(store.inner.tracks(lib.id).len()).unwrap();
    assert!(stored > 1, "a batch partway through B/Y was stored");
    assert_eq!((row.progress.files_seen, row.progress.added), (1, stored), "{:?}", row.progress);

    run(&store, &lib, &mut row, &batches_of(2), &Stops::default());
    assert_resumed_like_an_uninterrupted_scan(&store, &lib);
}

/// The resumed scan of [`two_folders`] completed with the progress an uninterrupted scan of the
/// same files reports, each of its five tracks stored once, and both problems recorded.
fn assert_resumed_like_an_uninterrupted_scan<F, L>(store: &Hooked<F, L>, lib: &LibraryConfig)
where
    F: Fn() -> Result<(), StoreError> + Send + Sync,
    L: Fn(&Path) -> Result<(), StoreError> + Send + Sync,
{
    let uninterrupted = scan_flushing_often(&MemoryStore::new(), lib);
    let resumed = only_row(store, lib);
    assert_eq!(resumed.state, ScanState::Completed);
    assert_eq!(resumed.progress, uninterrupted.progress);
    assert_eq!(
        (resumed.progress.files_seen, resumed.progress.added, resumed.progress.problems),
        (6, 5, 2),
        "{:?}",
        resumed.progress
    );
    let root = &lib.roots[0];
    let mut paths: Vec<_> = store.inner.tracks(lib.id).into_iter().map(|t| t.record.path).collect();
    paths.sort();
    let mut expected = vec![track(root, "A/X", 1)];
    expected.extend((1..=4).map(|n| track(root, "B/Y", n)));
    assert_eq!(paths, expected);
    assert_eq!(store.inner.problems(lib.id).len(), 2);
}

/// A known track that cannot be read is counted once across a resume, which reads it again. A/X's
/// four tracks are scanned, then the first is broken. A scan shut down as the third track is read,
/// after the first was persisted, resumes to the counts of one never stopped: the broken file
/// processed and a problem once, and the four files seen.
#[test]
fn a_known_file_that_cannot_be_read_is_counted_once_across_a_resume() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 4);
    let lib = library(&root);
    let stops = Stops::default();
    let stop_at = track(&root, "A/X", 3);
    let on_lookup = |path: &Path| {
        if path == stop_at {
            stops.shutdown.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    let store =
        Hooked { inner: MemoryStore::new(), on_apply: || Ok(()), on_lookup, on_under: || Ok(()) };
    scan_flushing_often(&store.inner, &lib);
    crate::testing::break_file(&track(&root, "A/X", 1));

    let mut scan = new_scan(&store, &lib);
    run(&store, &lib, &mut scan, &batches_of(1), &stops);
    let row = |id| store.inner.scans(lib.id).into_iter().find(|s| s.id == id).unwrap();
    let mut stopped = row(scan.id);
    assert_eq!(stopped.state, ScanState::Running);
    run(&store, &lib, &mut stopped, &batches_of(1), &Stops::default());

    let resumed = row(scan.id);
    let p = &resumed.progress;
    assert_eq!(resumed.state, ScanState::Completed);
    assert_eq!((p.files_seen, p.files_processed, p.problems), (4, 1, 1), "{p:?}");
    assert!(store.inner.tracks(lib.id).iter().all(|t| !t.missing));
}
