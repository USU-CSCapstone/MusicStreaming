//! The checkpoint before each file: a folder of many files flushes partway, and a cancel takes
//! effect partway through it (`design/scanning.md` §7, §9).

use super::*;

/// The number of tracks in each batch that writes any, in the order a scan with `options` of a
/// folder of five tracks applies them, and the scan as it ends.
fn tracks_per_batch<S: InspectStore + 'static>(
    make: Factory<S>,
    options: &ScanOptions,
) -> (Vec<usize>, Scan) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 5);
    let lib = library(&root);
    let batches = Mutex::new(Vec::new());
    let on_apply = |batch: &Batch| {
        if !batch.upserts.is_empty() {
            batches.lock().unwrap().push(batch.upserts.len());
        }
        Ok(())
    };
    let store = Hooked::new(Arc::new(make(&lib))).on_apply(on_apply);
    let mut scan = new_scan(&store, &lib);
    run(&store, &lib, &mut scan, options, &Stops::default());
    let per_batch = batches.lock().unwrap().clone();
    (per_batch, scan)
}

/// A folder of five tracks flushes at every second outcome, so its tracks are stored two, two
/// and one at a time.
pub fn a_folder_larger_than_a_batch_flushes_partway<S: InspectStore + 'static>(make: Factory<S>) {
    let (batches, scan) = tracks_per_batch(make, &batches_of(2));
    assert_eq!(batches, [2, 2, 1]);
    assert_eq!(scan.state, ScanState::Completed);
    assert_eq!((scan.progress.files_seen, scan.progress.added), (5, 5), "{:?}", scan.progress);
}

/// A batch far from full still flushes partway through a folder once the interval has passed.
pub fn a_folder_flushes_partway_once_the_interval_passes<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let options = ScanOptions { batch_size: 1000, batch_interval: Duration::ZERO };
    let (batches, scan) = tracks_per_batch(make, &options);
    assert!(batches.len() > 1, "the folder's tracks were stored in one batch: {batches:?}");
    assert_eq!(scan.progress.added, 5, "{:?}", scan.progress);
}

/// A cancel asked for as the batch holding a folder's second track is stored ends the scan before
/// the third, with the first two stored and the cursor still at the last folder completed. A
/// cancelled scan never resumes, so it counts what it saw of the folder: the broken file and two
/// tracks.
pub fn a_cancel_partway_through_a_folder_stops_there_and_counts_what_it_saw<
    S: InspectStore + 'static,
>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 5);
    fs::write(root.join("A/X/00 - Broken.flac"), b"not audio").unwrap();
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let stops = Stops::default();
    let cancel_at = track(&root, "A/X", 2);
    let on_apply = |batch: &Batch| {
        if batch.upserts.iter().any(|t| t.path == cancel_at) {
            stops.cancel.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_apply(on_apply);
    let mut scan = new_scan(&hooked, &lib);
    run(&hooked, &lib, &mut scan, &batches_of(1), &stops);

    let row = only_row(store.as_ref(), &lib);
    assert_eq!((row.state, &row.cursor), (ScanState::Cancelled, &after(&root, "A")), "{row:?}");
    let saw = ScanProgress {
        files_seen: 3,
        files_processed: 3,
        added: 2,
        problems: 1,
        ..Default::default()
    };
    assert_eq!(row.progress, saw);
    assert_eq!(store.inspect_tracks(lib.id).len(), 2);
}
