//! A scan stopped partway through a folder, which its cursor has not passed, resumes with every
//! track stored once and every problem recorded (`design/scanning.md` §9).

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

/// A shutdown asked for as the batch holding B/Y's second track is stored leaves the row running
/// at B, the last folder completed, with three tracks stored. The resumed scan ends with each of
/// the five tracks stored once and both problems recorded.
pub fn a_scan_shut_down_partway_through_a_folder_resumes_storing_each_track_once<
    S: InspectStore + 'static,
>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    two_folders(&root);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let stops = Stops::default();
    let stop_at = track(&root, "B/Y", 2);
    let on_apply = |batch: &Batch| {
        if batch.upserts.iter().any(|t| t.path == stop_at) {
            stops.shutdown.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_apply(on_apply);
    let mut scan = new_scan(&hooked, &lib);
    run(&hooked, &lib, &mut scan, &batches_of(2), &stops);

    let mut row = only_row(store.as_ref(), &lib);
    assert_eq!((row.state, &row.cursor), (ScanState::Running, &after(&root, "B")));
    assert_eq!(store.inspect_tracks(lib.id).len(), 3);

    run(&hooked, &lib, &mut row, &batches_of(2), &Stops::default());

    assert_eq!(only_row(store.as_ref(), &lib).state, ScanState::Completed);
    let mut expected = vec![track(&root, "A/X", 1)];
    expected.extend((1..=4).map(|n| track(&root, "B/Y", n)));
    assert_eq!(paths(store.as_ref(), &lib), expected);
    assert_eq!(store.inspect_problems(lib.id).len(), 2);
}
