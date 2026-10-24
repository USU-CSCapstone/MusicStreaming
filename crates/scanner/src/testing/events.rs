//! Scenarios for what the watcher's events make a scan do: the scopes an event dirties, run as
//! one watcher scan, move, keep or mark missing what the index holds (`design/scanning.md` §9).

use std::fs;
use std::path::{Path, PathBuf};

use notify::EventKind;
use notify::event::{ModifyKind, RemoveKind, RenameMode};

use super::{Factory, InspectStore, library, make_album, scan_library, scan_once};
use crate::scan::Trigger;
use crate::scope::{Depth, Scope};
use crate::triggers::classify;

/// The scopes `classify` makes of `events` in `root`.
fn scopes(root: &Path, events: &[(EventKind, Vec<PathBuf>)]) -> Vec<Scope> {
    let mut scopes: Vec<Scope> = events
        .iter()
        .flat_map(|(kind, paths)| paths.iter().flat_map(|path| classify(kind, path)))
        .map(|(dir, depth)| Scope::folder(root, dir, depth))
        .collect();
    scopes.sort_by(|a, b| a.path.cmp(&b.path));
    scopes.dedup();
    scopes
}

/// Runs the scopes `classify` makes of `events` as one watcher scan of `root`'s library.
fn scan_events(store: &dyn InspectStore, root: &Path, events: &[(EventKind, Vec<PathBuf>)]) {
    let scopes = scopes(root, events);
    scan_once(store, &library(root), Trigger::Watch, scopes);
}

/// Each track's path, and whether it is missing, sorted by path.
fn indexed(store: &dyn InspectStore) -> Vec<(PathBuf, bool)> {
    let mut tracks: Vec<(PathBuf, bool)> =
        store.inspect_tracks(1).into_iter().map(|t| (t.path, t.missing)).collect();
    tracks.sort();
    tracks
}

/// A folder renamed, with the events a Linux watcher reports for `mv A B`.
pub fn a_renamed_folder_moves_its_tracks<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "Artist", "A", 2);
    let store = make(&library(&root));
    scan_library(&store, &library(&root));
    let (a, b) = (root.join("Artist/A"), root.join("Artist/B"));
    fs::rename(&a, &b).unwrap();

    let name = |mode| EventKind::Modify(ModifyKind::Name(mode));
    scan_events(
        &store,
        &root,
        &[
            (name(RenameMode::From), vec![a.clone()]),
            (name(RenameMode::To), vec![b.clone()]),
            (name(RenameMode::Both), vec![a.clone(), b.clone()]),
        ],
    );
    let track = |dir: &Path, n: u32| dir.join(format!("{n:02} - Track {n}.wav"));
    assert_eq!(
        indexed(&store),
        [(track(&a, 1), true), (track(&a, 2), true), (track(&b, 1), false), (track(&b, 2), false)]
    );
}

/// A file renamed is a change to its folder only, as a file has no subtree to mark missing.
pub fn a_renamed_file_dirties_only_its_folder<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "Artist", "A", 1);
    let store = make(&library(&root));
    scan_library(&store, &library(&root));
    let a = root.join("Artist/A");
    let (old, new) = (a.join("01 - Track 1.wav"), a.join("renamed.wav"));
    fs::rename(&old, &new).unwrap();

    let name = |mode| EventKind::Modify(ModifyKind::Name(mode));
    let events = [(name(RenameMode::From), vec![old.clone()]), (name(RenameMode::To), vec![new])];
    assert_eq!(scopes(&root, &events), [Scope::folder(&root, &a, Depth::Directory)]);
}

/// A file deleted, then a folder deleted, with the events a Linux watcher reports for `rm`
/// and `rm -r`.
pub fn deleted_files_and_folders_go_missing<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "Artist", "A", 2);
    make_album(&root, "Artist", "D", 2);
    fs::write(root.join("Artist/D/broken.flac"), b"not audio").unwrap();
    let store = make(&library(&root));
    scan_library(&store, &library(&root));
    assert_eq!(store.inspect_problems(1).len(), 1);
    let (a, d) = (root.join("Artist/A"), root.join("Artist/D"));
    let track = |dir: &Path, n: u32| dir.join(format!("{n:02} - Track {n}.wav"));

    fs::remove_file(track(&a, 1)).unwrap();
    let removed = EventKind::Remove(RemoveKind::File);
    assert_eq!(classify(&removed, &track(&a, 1)), [(a.clone(), Depth::Directory)]);
    scan_events(&store, &root, &[(removed, vec![track(&a, 1)])]);
    assert_eq!(
        indexed(&store),
        [(track(&a, 1), true), (track(&a, 2), false), (track(&d, 1), false), (track(&d, 2), false)]
    );

    fs::remove_dir_all(&d).unwrap();
    scan_events(
        &store,
        &root,
        &[
            (EventKind::Remove(RemoveKind::File), vec![track(&d, 1)]),
            (EventKind::Remove(RemoveKind::File), vec![track(&d, 2)]),
            (EventKind::Remove(RemoveKind::Folder), vec![d.clone()]),
        ],
    );
    assert_eq!(
        indexed(&store),
        [(track(&a, 1), true), (track(&a, 2), false), (track(&d, 1), true), (track(&d, 2), true)]
    );
    assert_eq!(store.inspect_problems(1), [], "a deleted folder's problem stayed");
}
