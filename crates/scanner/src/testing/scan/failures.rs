//! A store that cannot answer partway through a scan ends it at its last persisted cursor
//! (`requirements/scanning.md` §10).

use super::*;
use crate::queue::Scanner;

/// A lookup that fails as B/Y's second track is reached suspends the scan at B, the last folder
/// persisted, before anything more is written: the file is never taken for a new one, which
/// would add a second track for its path.
pub fn a_lookup_the_store_fails_suspends_the_scan_before_another_write<
    S: InspectStore + 'static,
>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 3);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let failed = AtomicBool::new(false);
    let written_after = AtomicBool::new(false);
    let fail_at = track(&root, "B/Y", 2);
    let on_lookup = |path: &Path| {
        if path == fail_at {
            failed.store(true, Ordering::SeqCst);
            return Err(StoreError("database is locked".into()));
        }
        Ok(())
    };
    let on_apply = |_: &_| {
        if failed.load(Ordering::SeqCst) {
            written_after.store(true, Ordering::SeqCst);
            return Err(StoreError("UNIQUE constraint failed".into()));
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_apply(on_apply).on_lookup(on_lookup);
    scan_flushing_often(store.as_ref(), &lib);

    let mut scan = new_scan(&hooked, &lib);
    run(&hooked, &lib, &mut scan, &batches_of(1), &Stops::default());
    assert!(!written_after.load(Ordering::SeqCst), "a batch was written after the failure");
    let row = row(store.as_ref(), &lib, scan.id);
    assert_eq!((row.state, &row.cursor), (ScanState::Suspended, &after(&root, "B")), "{row:?}");
    assert_eq!(row.progress.added, 0, "{:?}", row.progress);
    let tracks = store.inspect_tracks(lib.id);
    assert_eq!(tracks.len(), 4);
    assert!(tracks.iter().all(|t| !t.missing));
}

/// A reconcile that cannot list the problems under its scope suspends the scan before marking
/// anything missing, so the next scan still finds the deleted file's track and counts it.
pub fn a_problem_list_the_store_fails_suspends_the_scan_before_marking_missing<
    S: InspectStore + 'static,
>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let hooked = Hooked::new(store.clone()).on_under(|| Err(StoreError("disk I/O error".into())));
    scan_flushing_often(store.as_ref(), &lib);
    fs::remove_file(track(&root, "A/X", 2)).unwrap();

    let scan = scan_flushing_often(&hooked, &lib);
    assert_eq!((scan.state, scan.progress.missing), (ScanState::Suspended, 0));
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));
    let scan = scan_flushing_often(store.as_ref(), &lib);
    assert_eq!((scan.state, scan.progress.missing), (ScanState::Completed, 1));
}

/// When the store cannot say whether anything is indexed under a folder, the scanner answers
/// yes, so the watcher scans the folder rather than miss a change in it.
pub fn a_folder_the_store_cannot_check_counts_as_indexed<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let store = Hooked::new(Arc::new(make(&library(&root))))
        .on_under(|| Err(StoreError("disk I/O error".into())));
    let (scanner, thread) = Scanner::start(Arc::new(store), library(&root)).unwrap();
    assert!(scanner.indexes_under(&root.join("A")));
    drop(thread);
}
