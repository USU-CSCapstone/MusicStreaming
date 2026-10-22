//! A store that cannot answer partway through a scan ends it at its last persisted cursor
//! (`requirements/scanning.md` §10).

use super::*;
use crate::queue::Scanner;

/// A read of what the index holds in B/Y that fails suspends the scan at A/X, the last folder
/// persisted with a track added there, before anything more is written: B/Y's files are never
/// taken for new ones, which would add a second track for each path.
pub fn a_folder_read_the_store_fails_suspends_the_scan_before_another_write<
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
    let fail_at = root.join("B/Y");
    let on_folder = |path: &Path| {
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
    let hooked = Hooked::new(store.clone()).on_apply(on_apply).on_folder(on_folder);
    scan_flushing_often(store.as_ref(), &lib);
    write_small_wav(&track(&root, "A/X", 2));

    let mut scan = new_scan(&hooked, &lib);
    run(&hooked, &lib, &mut scan, &batches_of(1), &Stops::default());
    assert!(!written_after.load(Ordering::SeqCst), "a batch was written after the failure");
    let row = row(store.as_ref(), &lib, scan.id);
    assert_eq!((row.state, &row.cursor), (ScanState::Suspended, &after(&root, "A/X")), "{row:?}");
    assert_eq!(row.progress.added, 1, "{:?}", row.progress);
    let tracks = store.inspect_tracks(lib.id);
    assert_eq!(tracks.len(), 5);
    assert!(tracks.iter().all(|t| !t.missing));
}

/// A reconcile that cannot list the problems under its scope suspends the scan, which counts the
/// deleted file's track it marked missing as it walked the file's folder, so the next scan finds
/// nothing more to mark.
pub fn a_problem_list_the_store_fails_suspends_the_scan_counting_what_it_marked_missing<
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
    assert_eq!((scan.state, scan.progress.missing), (ScanState::Suspended, 1));
    let missing = store.inspect_tracks(lib.id).into_iter().filter(|t| t.missing);
    assert_eq!(missing.map(|t| t.path).collect::<Vec<_>>(), [track(&root, "A/X", 2)]);
    let scan = scan_flushing_often(store.as_ref(), &lib);
    assert_eq!((scan.state, scan.progress.missing), (ScanState::Completed, 0));
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
