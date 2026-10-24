//! Scenarios for scopes: a scan of a folder, at either depth, reconciles only what lies inside it
//! (`design/scanning.md` §8, §9).

use std::fs;
use std::path::PathBuf;

use super::{
    Factory, InspectStore, library, live, make_album, scan_library, scan_once, write_small_wav,
};
use crate::scan::{ScanState, Trigger};
use crate::scope::{Depth, Scope};

/// A scope naming a symlinked folder walks nothing through it and records the link as a problem.
pub fn a_scope_through_a_folder_link_walks_nothing<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    // Another library's folder, linked into this one, as the watcher or a plugin may name it.
    make_album(&tmp.path().join("other"), "B", "Y", 1);
    let link = root.join("linked");
    std::os::unix::fs::symlink(tmp.path().join("other/B"), &link).unwrap();
    let lib = library(&root);
    let store = make(&lib);

    let scope = Scope::folder(&root, &link, Depth::Subtree);
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!(scan.progress.files_seen, 0);
    assert_eq!(scan.progress.problems, 1);
    let problems = store.inspect_problems(lib.id);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path, link);
    assert!(live(&store, &lib).is_empty());
}

/// A scope on a folder that no longer exists marks its tracks missing, takes the problems under
/// it with them, and records none of its own.
pub fn a_scope_naming_a_deleted_folder_reconciles<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    make_album(&root, "B", "Y", 1);
    fs::write(root.join("A/X/broken.flac"), b"not audio").unwrap();
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    assert_eq!(store.inspect_problems(lib.id).len(), 1);
    fs::remove_dir_all(root.join("A/X")).unwrap();

    let scope = Scope::folder(&root, root.join("A/X"), Depth::Subtree);
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!(scan.state, ScanState::Completed);
    assert_eq!(scan.progress.missing, 2);
    assert_eq!(scan.progress.problems, 0);
    assert_eq!(live(&store, &lib).len(), 1);
    assert_eq!(store.inspect_problems(lib.id), []);
}

/// A Subtree scope at a file, as the watcher makes when a folder is replaced by a file within
/// its wait, walks nothing: it records no problem, clears none, and marks nothing missing,
/// whether the file is there or not.
pub fn a_scope_at_a_file_reconciles_nothing<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    let track = root.join("A/X/01 - Track 1.wav");
    let scope = Scope::folder(&root, &track, Depth::Subtree);

    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope.clone()]);
    assert_eq!(
        (scan.state, scan.progress.problems, scan.progress.missing),
        (ScanState::Completed, 0, 0)
    );
    assert_eq!(store.inspect_problems(lib.id), []);

    fs::remove_file(&track).unwrap();
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!(scan.progress.missing, 0);
    assert_eq!(live(&store, &lib).len(), 1);

    let broken = root.join("A/X/broken.flac");
    fs::write(&broken, b"not audio").unwrap();
    scan_library(&store, &lib);
    let scope = Scope::folder(&root, &broken, Depth::Subtree);
    scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    let problems: Vec<PathBuf> =
        store.inspect_problems(lib.id).into_iter().map(|p| p.path).collect();
    assert_eq!(problems, [broken]);
}

/// A Subtree scope on a folder marks missing only the deleted files inside it.
pub fn folder_scope_reconciles_only_inside_itself<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);

    fs::remove_file(root.join("B/Y/01 - Track 1.wav")).unwrap();
    let scan = scan_once(
        &store,
        &lib,
        Trigger::Manual,
        vec![Scope::folder(&root, root.join("A"), Depth::Subtree)],
    );
    assert_eq!(scan.progress.missing, 0);
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));
    let scan = scan_once(
        &store,
        &lib,
        Trigger::Manual,
        vec![Scope::folder(&root, root.join("B"), Depth::Subtree)],
    );
    assert_eq!(scan.progress.missing, 1);
}

/// A Subtree scope on a non-ASCII folder, which is longer in bytes than in characters,
/// reconciles only inside that folder. `Björk 1 (Live)` sorts just before `Björk 1/` and
/// `Björk 10` just after it, so neither range bound can move by one character unnoticed.
pub fn non_ascii_subtree_reconciles_only_inside_itself<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "Björk 1", "Début", 2);
    make_album(&root, "Björk 1 (Live)", "X", 1);
    make_album(&root, "Björk 10", "X", 1);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);

    for path in ["Björk 1/Début", "Björk 1 (Live)/X", "Björk 10/X"] {
        fs::remove_file(root.join(path).join("01 - Track 1.wav")).unwrap();
    }
    let scope = Scope::folder(&root, root.join("Björk 1"), Depth::Subtree);
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!(scan.progress.missing, 1);
    let live: Vec<PathBuf> = live(&store, &lib).into_iter().map(|t| t.path).collect();
    assert_eq!(
        live,
        [
            root.join("Björk 1/Début/02 - Track 2.wav"),
            root.join("Björk 1 (Live)/X/01 - Track 1.wav"),
            root.join("Björk 10/X/01 - Track 1.wav"),
        ]
    );
}

/// A Directory scope under a non-ASCII folder reconciles its own files and none in the folder
/// below it, whose one-character name the folder's extra bytes would otherwise skip past.
pub fn non_ascii_directory_reconciles_only_itself<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "Björk", "Début", 2);
    make_album(&root, "Björk/Début", "1", 1);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);

    fs::remove_file(root.join("Björk/Début/01 - Track 1.wav")).unwrap();
    let scope = Scope::folder(&root, root.join("Björk/Début"), Depth::Directory);
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!(scan.progress.missing, 1);
    let live: Vec<PathBuf> = live(&store, &lib).into_iter().map(|t| t.path).collect();
    assert_eq!(
        live,
        [root.join("Björk/Début/02 - Track 2.wav"), root.join("Björk/Début/1/01 - Track 1.wav")]
    );
}

/// A Directory scope on the root reconciles the root's own files and none in its folders.
pub fn root_directory_reconciles_only_its_own_files<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    write_small_wav(&root.join("loose.wav"));
    make_album(&root, "A", "X", 1);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);

    fs::remove_file(root.join("loose.wav")).unwrap();
    let scope = Scope::folder(&root, &root, Depth::Directory);
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!(scan.progress.missing, 1);
    let live: Vec<PathBuf> = live(&store, &lib).into_iter().map(|t| t.path).collect();
    assert_eq!(live, [root.join("A/X/01 - Track 1.wav")]);
}
