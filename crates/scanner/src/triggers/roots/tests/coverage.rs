//! Which folders a root's watch covers, as it starts and as folders appear.

use std::fs::Permissions;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::mpsc::Receiver;

use super::*;
use crate::triggers::spawn_watch;

/// Writes a file into `folder`, and returns the paths of every event up to the first for that
/// file, failing past [`PATIENCE`].
fn events_until(events: &Receiver<notify::Result<notify::Event>>, folder: &Path) -> Vec<PathBuf> {
    let file = folder.join("new.txt");
    fs::write(&file, b"x").unwrap();
    let deadline = Instant::now() + PATIENCE;
    let mut paths = Vec::new();
    while !paths.contains(&file) {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = events.recv_timeout(left);
        let event = event.unwrap_or_else(|error| panic!("{error}: none in {}", folder.display()));
        paths.extend(event.unwrap().paths);
    }
    paths
}

/// Makes `folder` unreadable, or returns false, saying so, where permissions do not stop this
/// user reading, as for root.
fn lock(folder: &Path) -> bool {
    fs::set_permissions(folder, Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(folder).is_ok() {
        fs::set_permissions(folder, Permissions::from_mode(0o755)).unwrap();
        eprintln!("skipped: permissions do not stop this user reading a folder (running as root?)");
        return false;
    }
    true
}

/// Makes `folder` readable again.
fn unlock(folder: &Path) {
    fs::set_permissions(folder, Permissions::from_mode(0o755)).unwrap();
}

/// A folder that cannot be read, listed before its siblings as a root-owned `lost+found` may
/// be, is passed over as the scan passes over it, and every other folder is watched.
#[test]
fn a_folder_that_cannot_be_read_leaves_the_rest_watched() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let siblings: Vec<PathBuf> = ["b", "c", "d/e"].iter().map(|name| root.join(name)).collect();
    siblings.iter().for_each(|folder| fs::create_dir_all(folder).unwrap());
    // Made last, which tmpfs lists first.
    let locked = root.join("a-locked");
    fs::create_dir(&locked).unwrap();
    if !lock(&locked) {
        return;
    }
    let (tx, rx) = mpsc::channel();
    let (_alive, stop) = Stop::new();
    let watch = watched(&root, &tx, &stop);
    unlock(&locked);
    assert!(watch.coverage.is_ok(), "{:?}", watch.coverage);
    for sibling in &siblings {
        events_until(&rx, sibling);
    }
}

/// No folder is watched through a link, whether the link is there as the root is watched or is
/// made later.
#[test]
fn no_folder_is_watched_through_a_link() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, outside) = (tmp.path().join("root"), tmp.path().join("outside"));
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    let (early, later) = (root.join("early"), root.join("later"));
    symlink(&outside, &early).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_alive, stop) = Stop::new();
    let mut watch = watched(&root, &tx, &stop);
    symlink(&outside, &later).unwrap();
    watch.watch_folder(&root, &later, &stop).unwrap();
    // Once the event in the root arrives, any for this earlier write would have arrived too.
    fs::write(outside.join("01.flac"), b"x").unwrap();
    let paths = events_until(&rx, &root);
    let behind = [early.join("01.flac"), later.join("01.flac")];
    assert!(!paths.iter().any(|path| behind.contains(path)), "{paths:?}");
}

/// Folders made or moved in once the root is watched are watched, with the folders below them,
/// as their events arrive.
#[test]
fn folders_that_appear_later_are_watched() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::create_dir_all(tmp.path().join("outside/a/b")).unwrap();
    let (scanner, thread, store) = scanner(&root);
    let watcher =
        spawn_watch(scanner.clone(), vec![root.clone()], Arc::default(), timing()).unwrap();
    let mut last = seen(&store, &root, &root, 0);
    fs::create_dir(root.join("new")).unwrap();
    last = seen(&store, &root, &root.join("new"), last);
    fs::rename(tmp.path().join("outside/a"), root.join("a")).unwrap();
    seen(&store, &root, &root.join("a/b"), last);
    drop(watcher);
    drop(thread);
}

/// Once the OS drops events, every root is walked again, so a folder made meanwhile, whose
/// event was dropped, is watched.
#[test]
fn dropped_events_have_every_root_walked_again() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (scanner, thread, _) = scanner(&root);
    let (tx, rx) = mpsc::channel();
    let paths = slice::from_ref(&root);
    let (_alive, stop) = Stop::new();
    let mut roots = Roots::start(scanner.clone(), paths, tx, Arc::default(), &timing(), &stop);
    // Made without its event reaching `roots`, as if the OS dropped it.
    let late = root.join("late/deep");
    fs::create_dir_all(&late).unwrap();
    roots.watch_all(&stop);
    events_until(&rx, &late);
    drop(roots);
    drop(thread);
}
