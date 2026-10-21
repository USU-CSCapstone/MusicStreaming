//! Scenarios for track identity across moves, copies and replacements (`design/scanning.md` §6).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    Factory, InspectStore, InspectTrack, library, live, make_album, scan_library, tag_wav,
    write_sine_wav,
};
use crate::scan::Scan;
use crate::track::TrackId;

/// The one track [`rescan_after`] indexes, relative to the library root.
const TRACK: &str = "A/X/01 - Track 1.wav";
/// Where the move, copy and replace scenarios put a file.
const ELSEWHERE: &str = "B/Y/01 - Track 1.wav";

/// Index one track at [`TRACK`], apply `change` to the library root, and scan again.
/// Returns the track as first indexed, the second scan, every track after it, missing
/// ones included, and the tracks the second scan announced in the feed.
fn rescan_after<S: InspectStore>(
    make: Factory<S>,
    change: impl FnOnce(&Path),
) -> (InspectTrack, Scan, Vec<InspectTrack>, Vec<TrackId>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    let before = live(&store, &lib)[0].clone();
    let head = store.feed_head(lib.id);
    change(&root);
    let scan = scan_library(&store, &lib);
    (before, scan, store.inspect_tracks(lib.id), store.feed_tracks_after(lib.id, head))
}

/// Move [`TRACK`] under `root` to [`ELSEWHERE`], returning its new path.
fn move_track(root: &Path) -> PathBuf {
    let moved = root.join(ELSEWHERE);
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::rename(root.join(TRACK), &moved).unwrap();
    moved
}

/// Assert that `scan` found `before` again at [`ELSEWHERE`], adding and losing nothing,
/// and return it as it is now.
fn assert_found_moved<'a>(
    before: &InspectTrack,
    scan: &Scan,
    tracks: &'a [InspectTrack],
) -> &'a InspectTrack {
    assert_eq!(scan.progress.added, 0);
    assert_eq!(scan.progress.missing, 0);
    let [after] = tracks else { panic!("one track, not {tracks:?}") };
    assert!(!after.missing);
    assert!(after.path.ends_with(ELSEWHERE), "{:?}", after.path);
    assert_eq!(after.id, before.id);
    after
}

/// A file moved to another folder is the same track in its new place
/// (`requirements/scanning.md` §6).
pub fn move_keeps_identity<S: InspectStore>(make: Factory<S>) {
    let (before, scan, tracks, announced) = rescan_after(make, |root| {
        move_track(root);
    });
    assert_found_moved(&before, &scan, &tracks);
    assert!(announced.iter().all(|id| *id == before.id), "{announced:?}");
}

/// A file both moved and retagged before the next scan is still the same track, with
/// its new tags (`requirements/scanning.md` §6). The new title changes the file's size, so
/// matching on size alone does not pass.
pub fn move_and_retag_keeps_identity<S: InspectStore>(make: Factory<S>) {
    const TITLE: &str = "A title much longer than the one it replaces";
    let (before, scan, tracks, announced) = rescan_after(make, |root| {
        let moved = move_track(root);
        let size = fs::metadata(&moved).unwrap().len();
        tag_wav(&moved, TITLE, "B", "Y");
        assert_ne!(fs::metadata(&moved).unwrap().len(), size, "the retag changes the size");
        fs::File::open(&moved)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
            .unwrap();
    });
    let after = assert_found_moved(&before, &scan, &tracks);
    assert_eq!(after.title, TITLE);
    assert_eq!(announced, vec![before.id]);
}

/// A copy of a file is a second track, and the original keeps its identity
/// (`requirements/scanning.md` §9).
pub fn copy_is_a_second_track<S: InspectStore>(make: Factory<S>) {
    let (before, scan, tracks, _) = rescan_after(make, |root| {
        let copy = root.join(ELSEWHERE);
        fs::create_dir_all(copy.parent().unwrap()).unwrap();
        fs::copy(root.join(TRACK), copy).unwrap();
    });
    assert_eq!(scan.progress.added, 1);
    assert_eq!(scan.progress.missing, 0);
    assert!(tracks.iter().all(|t| !t.missing), "{tracks:?}");
    assert_eq!(tracks.len(), 2, "{tracks:?}");
    let at = |place: &str| tracks.iter().find(|t| t.path.ends_with(place)).unwrap();
    let (original, copy) = (at(TRACK), at(ELSEWHERE));
    assert_eq!(original.id, before.id);
    assert_ne!(copy.id, before.id);
}

/// A file deleted while different audio appears elsewhere is a loss and a new track, not a
/// move: the vanished track's identity does not pass to a file with other content
/// (`requirements/scanning.md` §6).
pub fn replaced_is_not_a_move<S: InspectStore>(make: Factory<S>) {
    let (before, scan, tracks, _) = rescan_after(make, |root| {
        fs::remove_file(root.join(TRACK)).unwrap();
        write_sine_wav(&root.join(ELSEWHERE), 880.0, 0.3, 0.5);
    });
    assert_eq!(scan.progress.added, 1);
    assert_eq!(scan.progress.missing, 1);
    let missing: Vec<&InspectTrack> = tracks.iter().filter(|t| t.missing).collect();
    assert_eq!(missing.len(), 1, "{tracks:?}");
    assert_eq!(missing[0].id, before.id);
}
