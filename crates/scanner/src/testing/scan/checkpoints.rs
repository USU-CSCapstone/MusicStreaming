//! The checkpoint before each file: a folder of many files flushes partway, at a full batch or
//! once the interval passes (`design/scanning.md` §7, §9).

use super::*;

/// The number of tracks in each batch that writes any, in the order a scan with `options` of a
/// folder of `tracks` tracks applies them, and the scan as it ends.
fn tracks_per_batch<S: InspectStore + 'static>(
    make: Factory<S>,
    options: &ScanOptions,
    tracks: usize,
) -> (Vec<usize>, Scan) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", tracks);
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

/// A folder of five tracks flushes whenever a batch of two is full, so its tracks are stored two,
/// two and one at a time.
pub fn a_folder_larger_than_a_batch_flushes_partway<S: InspectStore + 'static>(make: Factory<S>) {
    let (batches, scan) = tracks_per_batch(make, &batches_of(2), 5);
    assert_eq!(batches, [2, 2, 1]);
    assert_eq!(scan.state, ScanState::Completed);
    assert_eq!((scan.progress.files_seen, scan.progress.added), (5, 5), "{:?}", scan.progress);
}

/// A batch far from full still flushes partway through a folder once the interval has passed,
/// which with no interval is before every file.
pub fn a_folder_flushes_partway_once_the_interval_passes<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let options = ScanOptions { batch_interval: Duration::ZERO, ..batches_of(1000) };
    let (batches, scan) = tracks_per_batch(make, &options, 5);
    assert_eq!(batches, [1, 1, 1, 1, 1]);
    assert_eq!(scan.progress.added, 5, "{:?}", scan.progress);
}

/// With the scanner's own options a batch flushes at 32 tracks, so a folder of 70 is stored 32,
/// 32 and 6 at a time.
pub fn a_default_batch_holds_32_tracks<S: InspectStore + 'static>(make: Factory<S>) {
    let options = ScanOptions { batch_interval: Duration::MAX, ..ScanOptions::default() };
    let (batches, scan) = tracks_per_batch(make, &options, 70);
    assert_eq!(batches, [32, 32, 6]);
    assert_eq!(scan.progress.added, 70, "{:?}", scan.progress);
}
