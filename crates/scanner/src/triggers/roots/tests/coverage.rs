//! Which folders a root's watch covers, as it starts and as folders appear.

use std::fs::{DirBuilder, Permissions};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;

use super::*;
use crate::triggers::FsWatcher;

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
    let watch = watched(&root, &tx);
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
    let mut watch = watched(&root, &tx);
    symlink(&outside, &later).unwrap();
    watch.watch_folder(&root, &later).unwrap();
    // Once the event in the root arrives, any for this earlier write would have arrived too.
    fs::write(outside.join("01.flac"), b"x").unwrap();
    let paths = events_until(&rx, &root);
    let behind = [early.join("01.flac"), later.join("01.flac")];
    assert!(!paths.iter().any(|path| behind.contains(path)), "{paths:?}");
}

/// Folders made readable, made, or moved in once the root is watched are watched, with the
/// folders below them, as their events arrive.
#[test]
fn folders_that_appear_later_are_watched() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let locked = root.join("locked");
    fs::create_dir_all(&locked).unwrap();
    fs::create_dir_all(tmp.path().join("outside/a/b")).unwrap();
    let was_locked = lock(&locked);
    let (scanner, thread, store) = scanner(&root);
    let watcher =
        FsWatcher::spawn(scanner.clone(), vec![root.clone()], watched, timing(None)).unwrap();
    let mut last = seen(&store, &root, &root, 0);
    if was_locked {
        unlock(&locked);
        last = seen(&store, &root, &locked, last);
    }
    fs::create_dir(root.join("new")).unwrap();
    last = seen(&store, &root, &root.join("new"), last);
    fs::rename(tmp.path().join("outside/a"), root.join("a")).unwrap();
    seen(&store, &root, &root.join("a/b"), last);
    drop(watcher);
    drop(thread);
}

/// A change of permissions, as `chmod -R` makes, walks nothing again but folders that could not
/// be read: nothing while every folder is readable, one try at the unreadable one for a change
/// to the root, which holds it, and the folder itself once it is made readable, after which
/// it is not tried again.
#[test]
fn a_change_of_permissions_tries_only_unreadable_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let readable: Vec<PathBuf> = ["", "a", "a/b", "c"].iter().map(|f| root.join(f)).collect();
    fs::create_dir_all(root.join("a/b")).unwrap();
    fs::create_dir(root.join("c")).unwrap();
    let chmod = |folders: &[PathBuf]| folders.iter().for_each(|folder| unlock(folder));
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let watch = move |root: &Path, events: &Events| FailsAt::counted(root, events, &counted);
    let (scanner, thread, store) = scanner(&root);
    let watcher =
        FsWatcher::spawn(scanner.clone(), vec![root.clone()], watch, timing(None)).unwrap();
    let calls = || calls.load(Ordering::SeqCst);
    // Each `seen` follows the changes before it, as a watch delivers events in order.
    let mut last = seen(&store, &root, &root, 0);
    let before = calls();
    chmod(&readable);
    last = seen(&store, &root, &root, last);
    assert_eq!(calls(), before, "readable folders were walked again");

    let locked = root.join("locked");
    DirBuilder::new().mode(0o000).create(&locked).unwrap();
    if fs::read_dir(&locked).is_ok() {
        eprintln!("skipped: permissions do not stop this user reading a folder (running as root?)");
    } else {
        last = seen(&store, &root, &root, last);
        let before = calls();
        chmod(&readable[1..]);
        last = seen(&store, &root, &root, last);
        let below = calls() - before;
        chmod(&readable[..1]);
        last = seen(&store, &root, &root, last);
        let at_root = calls() - before - below;
        unlock(&locked);
        assert_eq!((below, at_root), (0, 1), "folders tried again: below the root, at it");
        last = seen(&store, &root, &locked, last);
        // Once watched, the folder is not tried again.
        let before = calls();
        chmod(&readable[..1]);
        seen(&store, &root, &root, last);
        assert_eq!(calls(), before, "a folder made readable was tried again");
    }
    drop(watcher);
    drop(thread);
}

/// A folder made once the OS limit on watches is reached cannot be watched, so the library is
/// no longer wholly watched. The root is not watched afresh, as the limit would stop that too.
#[test]
fn reaching_the_limit_later_starts_the_fallback_schedule() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let new = root.join("new");
    let (scanner, thread, _) = scanner(&root);
    let tries = Cell::new(0);
    let limit_at_new = |root: &Path, events: &Events| {
        tries.set(tries.get() + 1);
        FailsAt::watch(root, events, root.join("new"), limit_error)
    };
    let start = Instant::now();
    let roots_at = slice::from_ref(&root);
    let mut roots =
        Roots::start(scanner.clone(), roots_at, mpsc::channel().0, limit_at_new, &seconds());
    assert!(roots.fallback.is_none());
    fs::create_dir(&new).unwrap();
    roots.watch_new(&new);
    assert!(roots.fallback.is_some());
    roots.check(start + 2 * MAX_BACKOFF);
    assert_eq!(tries.get(), 1, "a root the limit stopped was watched afresh");
    drop(roots);
    drop(thread);
}

/// A folder made later that cannot be watched, short of the OS limit, leaves the library not
/// wholly watched until a check watches its root afresh, which scans it.
#[test]
fn a_folder_that_fails_later_has_its_root_watched_afresh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let new = root.join("new");
    let (scanner, thread, store) = scanner(&root);
    let tries = Cell::new(0);
    // The first watch fails at the new folder; a fresh one fails nowhere.
    let fails_once = |root: &Path, events: &Events| {
        tries.set(tries.get() + 1);
        let folder = if tries.get() == 1 { root.join("new") } else { PathBuf::new() };
        FailsAt::watch(root, events, folder, || notify::Error::generic("failed"))
    };
    let start = Instant::now();
    let roots_at = slice::from_ref(&root);
    let mut roots =
        Roots::start(scanner.clone(), roots_at, mpsc::channel().0, fails_once, &seconds());
    fs::create_dir(&new).unwrap();
    roots.watch_new(&new);
    assert!(roots.fallback.is_some());
    roots.check(start + Duration::from_millis(1500));
    assert_eq!(tries.get(), 2);
    assert!(roots.fallback.is_none(), "the fallback schedule outlived the fresh watch");
    let scopes: Vec<Vec<Scope>> = scans(&store).into_iter().map(|scan| scan.scopes).collect();
    assert_eq!(scopes, [vec![Scope::root(root.clone())]]);
    drop(roots);
    drop(thread);
}
