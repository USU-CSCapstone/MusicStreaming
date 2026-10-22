//! Excludes apply to every scope a scan is given, and a pattern that does not compile stops the
//! scan before it touches anything (`requirements/libraries.md` §1, `requirements/scanning.md`
//! §7).

use std::os::unix::fs::symlink;

use super::*;
use crate::scope::Depth;

/// A library at `root` with A/X and backup/X, one track each, excluding the folder `backup`
/// alone, so only an ancestor of backup/X matches.
fn library_with_backup(root: &Path) -> LibraryConfig {
    make_album(root, "A", "X", 1);
    make_album(root, "backup", "X", 1);
    LibraryConfig { excludes: vec!["backup".into()], ..library(root) }
}

/// A scan of `scopes`, as the watcher or a plugin names them, run to its end against `store`.
fn watch_scan(store: &dyn Store, lib: &LibraryConfig, scopes: &[Scope]) -> Scan {
    let mut scan = store.create_scan(lib.id, Trigger::Watch, scopes).unwrap();
    run(store, lib, &mut scan, &batches_of(1), &Stops::default());
    scan
}

/// A Directory scope inside the excluded folder indexes nothing, and the scope after it is still
/// scanned.
pub fn a_scope_inside_an_excluded_folder_indexes_nothing<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let lib = library_with_backup(&root);
    let store = make(&lib);
    let scopes = [
        Scope::folder(&root, root.join("backup/X"), Depth::Directory),
        Scope::folder(&root, root.join("A/X"), Depth::Directory),
    ];
    let scan = watch_scan(&store, &lib, &scopes);

    assert_eq!((scan.state, scan.progress.files_seen), (ScanState::Completed, 1), "{scan:?}");
    assert_eq!(paths(&store, &lib), [track(&root, "A/X", 1)]);
}

/// Tracks indexed before their folder was excluded are marked missing by a scope inside it, as a
/// scan of the whole library marks them (`requirements/scanning.md` §7).
pub fn a_scope_inside_a_newly_excluded_folder_marks_its_tracks_missing<S: InspectStore>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let lib = library_with_backup(&root);
    let store = make(&lib);
    scan_flushing_often(&store, &library(&root));

    let scope = Scope::folder(&root, root.join("backup/X"), Depth::Directory);
    let scan = watch_scan(&store, &lib, &[scope]);
    assert_eq!((scan.state, scan.progress.missing), (ScanState::Completed, 1), "{scan:?}");
    let tracks = store.inspect_tracks(lib.id).into_iter();
    let missing: Vec<_> = tracks.filter(|t| t.missing).map(|t| t.path).collect();
    assert_eq!(missing, [track(&root, "backup/X", 1)]);
}

/// A scope reached through a link inside the excluded folder records no problem, since excluded
/// content is never reported.
pub fn a_linked_scope_inside_an_excluded_folder_reports_nothing<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let lib = library_with_backup(&root);
    make_album(&tmp.path().join("elsewhere"), "B", "Y", 1);
    symlink(tmp.path().join("elsewhere/B"), root.join("backup/B")).unwrap();
    let store = make(&lib);

    let scope = Scope::folder(&root, root.join("backup/B/Y"), Depth::Directory);
    let scan = watch_scan(&store, &lib, &[scope]);
    assert_eq!((scan.state, scan.progress.problems), (ScanState::Completed, 0), "{scan:?}");
    assert!(store.inspect_problems(lib.id).is_empty());
    assert!(store.inspect_tracks(lib.id).is_empty());
}

/// An exclude pattern that does not compile suspends the scan before it touches anything: a file
/// added since the last scan is not indexed, and one deleted is not marked missing.
pub fn an_invalid_pattern_suspends_and_leaves_the_store_untouched<S: InspectStore>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    let lib = LibraryConfig { excludes: vec!["[".into()], ..library(&root) };
    let store = make(&lib);
    scan_flushing_often(&store, &library(&root));
    let tracks = || -> Vec<_> {
        let tracks = store.inspect_tracks(lib.id).into_iter();
        tracks.map(|t| (t.path, t.missing)).collect()
    };
    let before = tracks();

    make_album(&root, "B", "Y", 1);
    fs::remove_file(track(&root, "A/X", 1)).unwrap();
    let scan = scan_flushing_often(&store, &lib);

    assert_eq!((scan.state, &scan.cursor), (ScanState::Suspended, &None), "{scan:?}");
    assert_eq!(scan.progress, ScanProgress::default());
    let row = row(&store, &lib, scan.id);
    assert_eq!((row.state, row.started_at), (ScanState::Suspended, None));
    assert!(row.finished_at.is_some());
    assert_eq!(tracks(), before);
    assert!(store.inspect_problems(lib.id).is_empty());
}
