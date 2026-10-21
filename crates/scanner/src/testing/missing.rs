//! Scenarios for files and roots that go away: a file is marked missing and comes back as the
//! same track, while a root that does not answer touches nothing (`design/scanning.md` §8).

use std::fs;

use super::{Factory, InspectStore, InspectTrack, library, live, make_album, scan_library};
use crate::scan::ScanState;

/// A deleted file is marked missing and announced, and when it comes back unchanged it is the same
/// track again, announced once more.
pub fn missing_then_returned<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);

    let p = root.join("A/X/02 - Track 2.wav");
    let bytes = fs::read(&p).unwrap();
    let meta = fs::metadata(&p).unwrap();
    let head_before_missing = store.feed_head(lib.id);
    fs::remove_file(&p).unwrap();

    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.missing, 1);
    let missing: Vec<InspectTrack> =
        store.inspect_tracks(lib.id).into_iter().filter(|t| t.missing).collect();
    assert_eq!(missing.len(), 1);
    assert!(missing[0].path.ends_with("02 - Track 2.wav"));
    let id = missing[0].id;
    assert_eq!(
        store.feed_tracks_after(lib.id, head_before_missing),
        vec![id],
        "the loss is announced"
    );
    let head_before_return = store.feed_head(lib.id);

    // The file comes back byte-identical with its old mtime: reconnects.
    fs::write(&p, &bytes).unwrap();
    fs::File::open(&p).unwrap().set_modified(meta.modified().unwrap()).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.missing, 0);
    assert_eq!(scan.progress.added, 0);
    let t = store.inspect_tracks(lib.id).into_iter().find(|t| t.path == p).unwrap();
    assert!(!t.missing);
    assert_eq!(t.id, id);
    assert_eq!(
        store.feed_tracks_after(lib.id, head_before_return),
        vec![id],
        "the return is announced"
    );
    assert!(store.feed_head(lib.id) > head_before_return, "the head moves past the return");
}

/// A root that is gone, or an empty mount point, suspends the scan without marking anything
/// missing, and once the root is back a scan completes without adding anything.
pub fn unavailable_root_suspends_and_touches_nothing<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("nas");
    make_album(&root, "A", "X", 2);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    assert_eq!(live(&store, &lib).len(), 2);

    // "Unmount": the root vanishes.
    let stash = tmp.path().join("stash");
    fs::rename(&root, &stash).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.state, ScanState::Suspended);
    assert_eq!(scan.progress.missing, 0);
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));

    // An empty mount point is unavailable too.
    fs::create_dir(&root).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.state, ScanState::Suspended);
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));

    // Remount: everything unchanged, nothing re-added.
    fs::remove_dir(&root).unwrap();
    fs::rename(&stash, &root).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.state, ScanState::Completed);
    assert_eq!(scan.progress.added, 0);
}
