//! The checkpoint before each file: a folder of many files flushes partway, in batches sized to
//! how fast the store applies them, and a cancel takes effect partway through it
//! (`design/scanning.md` §7, §9).

use std::thread;

use super::*;

/// The number of tracks in each batch that writes any, in the order a scan with `options` of a
/// folder of five tracks applies them to a store slowed by `per_outcome` for each outcome in a
/// batch, and the scan as it ends.
fn tracks_per_batch<S: InspectStore + 'static>(
    make: Factory<S>,
    options: &ScanOptions,
    per_outcome: Duration,
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
        thread::sleep(per_outcome * u32::try_from(batch.size()).unwrap());
        Ok(())
    };
    let store = Hooked::new(Arc::new(make(&lib))).on_apply(on_apply);
    let mut scan = new_scan(&store, &lib);
    run(&store, &lib, &mut scan, options, &Stops::default());
    let per_batch = batches.lock().unwrap().clone();
    (per_batch, scan)
}

/// A folder of five tracks flushes at its first outcome, before the store has been measured,
/// and then at every second, so its tracks are stored one, two and two at a time.
pub fn a_folder_larger_than_a_batch_flushes_partway<S: InspectStore + 'static>(make: Factory<S>) {
    let (batches, scan) = tracks_per_batch(make, &batches_of(2), Duration::ZERO);
    assert_eq!(batches, [1, 2, 2]);
    assert_eq!(scan.state, ScanState::Completed);
    assert_eq!((scan.progress.files_seen, scan.progress.added), (5, 5), "{:?}", scan.progress);
}

/// A scan's first batch, before the store has been measured, holds `min_batch` outcomes, and
/// the next, sized by a time no batch can exceed, holds the rest. A `min_batch` over
/// `batch_size` gives way to it, from the first batch on.
pub fn a_scans_first_batch_holds_min_batch<S: InspectStore + 'static>(make: Factory<S>) {
    let options = ScanOptions { min_batch: 3, ..batches_of(1000) };
    let (batches, scan) = tracks_per_batch(make, &options, Duration::ZERO);
    assert_eq!(batches, [3, 2]);
    assert_eq!(scan.progress.added, 5, "{:?}", scan.progress);

    let options = ScanOptions { min_batch: 3, ..batches_of(2) };
    let (batches, scan) = tracks_per_batch(make, &options, Duration::ZERO);
    assert_eq!(batches, [2, 2, 1]);
    assert_eq!(scan.progress.added, 5, "{:?}", scan.progress);
}

/// A batch far from full still flushes partway through a folder once the interval has passed,
/// which with no interval is before every file.
pub fn a_folder_flushes_partway_once_the_interval_passes<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let options = ScanOptions { batch_interval: Duration::ZERO, ..batches_of(1000) };
    let (batches, scan) = tracks_per_batch(make, &options, Duration::ZERO);
    assert_eq!(batches, [1, 1, 1, 1, 1]);
    assert_eq!(scan.progress.added, 5, "{:?}", scan.progress);
}

/// A store that takes 10 ms for each outcome is given batches it applies within the 25 ms a
/// batch may take, so none holds more than two of the folder's tracks, and all five are stored.
pub fn a_slow_store_is_given_batches_it_applies_in_time<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let options = ScanOptions { batch_time: Duration::from_millis(25), ..batches_of(1000) };
    let (batches, scan) = tracks_per_batch(make, &options, Duration::from_millis(10));
    assert!(batches.iter().all(|&tracks| tracks <= 2), "a batch took too long: {batches:?}");
    assert_eq!(scan.state, ScanState::Completed);
    assert_eq!((scan.progress.files_seen, scan.progress.added), (5, 5), "{:?}", scan.progress);
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
