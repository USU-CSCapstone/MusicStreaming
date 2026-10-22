//! Scenarios for sidecars written or removed beside tracks whose files did not change, as a
//! plugin writes them (`requirements/scanning.md` §3, §7, `design/scanning.md` §2).

use std::fs;

use super::{Factory, InspectStore, PNG, library, live, make_album, scan_library, scan_once};
use crate::scan::Trigger;
use crate::scope::{Depth, Scope};

/// An empty lyrics file beside a track is no lyrics, and a rescan finds the track unchanged and
/// writes nothing.
pub fn an_empty_lyrics_file_writes_nothing<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    fs::write(root.join("A/X/01 - Track 1.lrc"), "").unwrap();
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    assert!(live(&store, &lib).iter().all(|t| !t.has_lyrics));
    let head = store.feed_head(lib.id);

    let again = scan_library(&store, &lib);
    assert_eq!(again.progress.updated, 0, "{:?}", again.progress);
    assert_eq!(store.feed_head(lib.id), head, "nothing written");
}

/// A cover and a biography written beside tracks whose files did not change reach every track
/// that resolves them on the next scan, stay put after that, and go when they are removed.
pub fn sidecars_beside_unchanged_tracks_come_and_go<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    make_album(&root, "A", "Y", 1);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    assert!(live(&store, &lib).iter().all(|t| !t.has_artwork && !t.has_biography));

    let (cover, biography) = (root.join("A/X/cover.png"), root.join("A/artist.txt"));
    fs::write(&cover, PNG).unwrap();
    fs::write(&biography, "A band.").unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.updated, 3, "each track below either: {:?}", scan.progress);
    let tracks = live(&store, &lib);
    let art: Vec<bool> = tracks.iter().map(|t| t.has_artwork).collect();
    assert_eq!(art, [true, true, false], "the cover is X's alone: {tracks:?}");
    assert!(tracks.iter().all(|t| t.has_biography), "{tracks:?}");
    assert_eq!(scan_library(&store, &lib).progress.updated, 0, "and stays incremental");

    fs::remove_file(&cover).unwrap();
    fs::remove_file(&biography).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.updated, 3, "{:?}", scan.progress);
    assert!(live(&store, &lib).iter().all(|t| !t.has_artwork && !t.has_biography));
}

/// A folder-scoped scan lists only its own folder, and locates the sidecars above it by listing
/// each folder up to the root itself. It stamps its unchanged tracks as a whole scan did, so it
/// writes nothing: here a cover at the root, under an empty `cover.jpg` that gives way to it, and
/// a biography in the artist's folder.
pub fn a_folder_scan_under_sidecars_finds_its_tracks_unchanged<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    fs::write(root.join("cover.png"), PNG).unwrap();
    fs::write(root.join("A/cover.jpg"), "").unwrap();
    fs::write(root.join("A/artist.txt"), "A band.").unwrap();
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    assert!(live(&store, &lib).iter().all(|t| t.has_artwork && t.has_biography));
    let head = store.feed_head(lib.id);

    let scope = Scope::folder(&root, root.join("A/X"), Depth::Directory);
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!((scan.progress.files_seen, scan.progress.updated), (2, 0), "{:?}", scan.progress);
    assert_eq!(store.feed_head(lib.id), head, "nothing written");
}
