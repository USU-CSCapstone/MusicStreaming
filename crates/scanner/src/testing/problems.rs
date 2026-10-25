//! Scenarios for problems: what the walk records, and when a problem clears
//! (`design/scanning.md` §8, §13).

use std::fs;
use std::path::PathBuf;

use super::{
    Factory, InspectStore, library, live, make_album, scan_library, scan_once, write_small_wav,
};
use crate::problems::{Problem, ProblemKind};
use crate::scan::{ScanState, Trigger};
use crate::scope::{Depth, Scope};
use crate::store::Batch;

/// A file that is not audio is recorded as corrupt audio without stopping the scan, and the
/// problem clears once a real file replaces it.
pub fn bad_file_is_a_problem_that_clears<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let bad = root.join("A/X/broken.flac");
    fs::write(&bad, b"this is not audio at all, just text pretending").unwrap();
    let lib = library(&root);
    let store = make(&lib);

    let scan = scan_library(&store, &lib);
    assert_eq!(scan.state, ScanState::Completed, "one bad file never aborts a scan");
    assert_eq!(scan.progress.added, 1);
    assert_eq!(scan.progress.problems, 1);
    let problems = store.inspect_problems(lib.id);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path, bad);
    assert_eq!(problems[0].kind, ProblemKind::CorruptAudio);

    // Replace with a real file: the problem clears.
    write_small_wav(&bad);
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.added, 1);
    assert!(store.inspect_problems(lib.id).is_empty());
}

/// A symlinked file is not followed: it is recorded as a problem and not indexed.
pub fn a_symlink_is_a_problem<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let outside = tmp.path().join("outside.wav");
    write_small_wav(&outside);
    let link = root.join("A/X/linked.wav");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let lib = library(&root);
    let store = make(&lib);

    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.added, 1, "only the regular file");
    assert_eq!(scan.progress.problems, 1);
    let problems = store.inspect_problems(lib.id);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path, link);
    assert_eq!(problems[0].detail, "symlinks are not followed");
}

/// A deleted file takes its problem with it, while a deleted file the scope does not reach, in a
/// sibling folder whose name extends the scope's or in a folder below it, keeps its problem
/// until its own folder is scanned.
pub fn a_deleted_file_takes_its_problem<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    for folder in ["A/X", "A/X 2", "A/X/Y"] {
        fs::create_dir_all(root.join(folder)).unwrap();
        fs::write(root.join(folder).join("broken.flac"), b"not audio").unwrap();
    }
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    assert_eq!(store.inspect_problems(lib.id).len(), 3);

    for folder in ["A/X", "A/X 2", "A/X/Y"] {
        fs::remove_file(root.join(folder).join("broken.flac")).unwrap();
    }
    let scope = Scope::folder(&root, root.join("A/X"), Depth::Directory);
    scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    let mut problems: Vec<PathBuf> =
        store.inspect_problems(lib.id).into_iter().map(|p| p.path).collect();
    problems.sort();
    assert_eq!(problems, [root.join("A/X/Y/broken.flac"), root.join("A/X 2/broken.flac")]);
    assert_eq!(live(&store, &lib).len(), 1);
}

/// A symlink that leads nowhere keeps its problem: the link is there, though its target is not.
pub fn a_dangling_link_keeps_its_problem<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let link = root.join("A/X/dangling.flac");
    std::os::unix::fs::symlink(root.join("A/X/nowhere.flac"), &link).unwrap();
    let lib = library(&root);
    let store = make(&lib);

    scan_library(&store, &lib);
    let problems: Vec<PathBuf> =
        store.inspect_problems(lib.id).into_iter().map(|p| p.path).collect();
    assert_eq!(problems, [link]);
}

/// A problem the walk cannot see the cause of, such as one recorded against a track by its
/// analysis, stays while the track's file is there.
pub fn a_problem_on_a_present_file_stays<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    let track = root.join("A/X/01 - Track 1.wav");
    let mut batch = Batch::default();
    batch.problems.push(Problem {
        path: track.clone(),
        kind: ProblemKind::CorruptAudio,
        detail: "decode failed".into(),
        seen_at: 0,
    });
    store.apply(lib.id, batch).unwrap();

    scan_library(&store, &lib);
    let problems: Vec<PathBuf> =
        store.inspect_problems(lib.id).into_iter().map(|p| p.path).collect();
    assert_eq!(problems, [track]);
}

/// A folder that could not be listed loses its problem once it lists. Skipped, saying so, where
/// permissions do not stop this user reading, as for root.
pub fn a_folder_problem_clears_once_it_lists<S: InspectStore>(make: Factory<S>) {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let lib = library(&root);
    let store = make(&lib);
    let folder = root.join("A/X");
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&folder).is_ok() {
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("skipped: permissions do not stop this user reading a folder (running as root?)");
        return;
    }
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.problems, 1);
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
    let problems: Vec<PathBuf> =
        store.inspect_problems(lib.id).into_iter().map(|p| p.path).collect();
    assert_eq!(problems, [folder]);

    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.added, 1);
    assert_eq!(store.inspect_problems(lib.id), []);
}

/// A folder that cannot be listed, and one whose entries cannot be read, keep their tracks,
/// those of a folder among those entries too, and record problems, while reconcile still runs
/// everywhere else (`requirements/scanning.md` §7). Skipped, saying so, where permissions do not
/// stop this user reading, as for root.
pub fn an_unreadable_folder_keeps_its_tracks<S: InspectStore>(make: Factory<S>) {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 2);
    // A sibling whose name extends B/Y's, which a text prefix would wrongly match.
    make_album(&root, "B", "Yz", 2);
    make_album(&root, "C", "Z", 2);
    make_album(&root, "C/Z", "Bonus", 1);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    fs::remove_file(root.join("A/X/01 - Track 1.wav")).unwrap();
    fs::remove_file(root.join("B/Yz/01 - Track 1.wav")).unwrap();

    let set_mode = |folder: &str, mode| {
        fs::set_permissions(root.join(folder), fs::Permissions::from_mode(mode)).unwrap()
    };
    // B/Y cannot be listed; C/Z lists its names, but none of them can be read.
    set_mode("B/Y", 0o000);
    set_mode("C/Z", 0o400);
    if fs::read_dir(root.join("B/Y")).is_ok() {
        set_mode("B/Y", 0o755);
        set_mode("C/Z", 0o755);
        eprintln!("skipped: permissions do not stop this user reading a folder (running as root?)");
        return;
    }
    let scan = scan_library(&store, &lib);
    set_mode("B/Y", 0o755);
    set_mode("C/Z", 0o755);

    assert_eq!(scan.state, ScanState::Completed);
    assert_eq!(scan.progress.missing, 2, "only the deleted files");
    assert_eq!(live(&store, &lib).len(), 6);
    let mut problems: Vec<(PathBuf, ProblemKind)> =
        store.inspect_problems(lib.id).into_iter().map(|p| (p.path, p.kind)).collect();
    problems.sort_by(|a, b| a.0.cmp(&b.0));
    let denied = |path: &str| (root.join(path), ProblemKind::PermissionDenied);
    assert_eq!(
        problems,
        [
            denied("B/Y"),
            denied("C/Z/01 - Track 1.wav"),
            denied("C/Z/02 - Track 2.wav"),
            denied("C/Z/Bonus"),
        ]
    );
}
