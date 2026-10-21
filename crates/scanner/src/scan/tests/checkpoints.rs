//! The checkpoint before each file: a folder of many files flushes partway, and a cancel takes
//! effect partway through it (`design/scanning.md` §7, §9).

use super::*;

/// How many batches a scan with `options` of a folder of five tracks applies before it reads the
/// last, and the scan as it ends.
fn applied_before_last(options: &ScanOptions) -> (usize, Scan) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 5);
    let lib = library(&root);
    let applied = AtomicUsize::new(0);
    let applied_before_last = AtomicUsize::new(usize::MAX);
    let last = track(&root, "A/X", 5);
    let on_apply = || {
        applied.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };
    let on_lookup = |path: &Path| {
        if path == last {
            applied_before_last.store(applied.load(Ordering::SeqCst), Ordering::SeqCst);
        }
        Ok(())
    };
    let store = Hooked {
        inner: MemoryStore::new(),
        on_apply,
        on_lookup,
        on_under: || Ok(()),
        on_scan_write: || Ok(()),
    };
    let mut scan = new_scan(&store, &lib);
    run(&store, &lib, &mut scan, options, &Stops::default());
    (applied_before_last.load(Ordering::SeqCst), scan)
}

/// A folder of five tracks flushes at every second outcome, so two batches are stored before
/// its last track is read.
#[test]
fn a_folder_larger_than_a_batch_flushes_partway() {
    let (applied, scan) = applied_before_last(&batches_of(2));
    assert_eq!(applied, 2);
    assert_eq!(scan.state, ScanState::Completed);
    assert_eq!((scan.progress.files_seen, scan.progress.added), (5, 5), "{:?}", scan.progress);
}

/// A batch far from full still flushes partway through a folder once the interval has passed.
#[test]
fn a_folder_flushes_partway_once_the_interval_passes() {
    let options = ScanOptions { batch_size: 1000, batch_interval: Duration::ZERO };
    let (applied, scan) = applied_before_last(&options);
    assert!(applied > 0, "no batch applied before the last track was read");
    assert_eq!(scan.progress.added, 5, "{:?}", scan.progress);
}

/// A cancel asked for as a folder's second track is read ends the scan before the third, with
/// the first two stored and the cursor still at the last folder completed. A cancelled scan never
/// resumes, so it counts what it saw of the folder: the broken file and two tracks.
#[test]
fn a_cancel_partway_through_a_folder_stops_there_and_counts_what_it_saw() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 5);
    fs::write(root.join("A/X/00 - Broken.flac"), b"not audio").unwrap();
    let lib = library(&root);
    let stops = Stops::default();
    let looked_up = AtomicUsize::new(0);
    let cancel_at = track(&root, "A/X", 2);
    let on_lookup = |path: &Path| {
        looked_up.fetch_add(1, Ordering::SeqCst);
        if path == cancel_at {
            stops.cancel.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    let store = Hooked {
        inner: MemoryStore::new(),
        on_apply: || Ok(()),
        on_lookup,
        on_under: || Ok(()),
        on_scan_write: || Ok(()),
    };
    let mut scan = new_scan(&store, &lib);
    run(&store, &lib, &mut scan, &batches_of(1000), &stops);

    let row = only_row(&store, &lib);
    assert_eq!((row.state, &row.cursor), (ScanState::Cancelled, &after(&root, "A")), "{row:?}");
    let saw = ScanProgress {
        files_seen: 3,
        files_processed: 3,
        added: 2,
        problems: 1,
        ..Default::default()
    };
    assert_eq!(row.progress, saw);
    assert_eq!(looked_up.load(Ordering::SeqCst), 3);
    assert_eq!(store.inner.tracks(lib.id).len(), 2);
}
