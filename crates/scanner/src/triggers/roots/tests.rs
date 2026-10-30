use std::slice;
use std::sync::{Arc, mpsc};

use super::*;
use crate::queue::ScannerThread;
use crate::scan::Scan;
use crate::scope::Depth;
use crate::testing::{BareStore, library};
use crate::worker::Stop;

mod coverage;
mod watcher;

/// How long a test waits for what it expects before it fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// A temporary folder, and its real path, which is the one events name: on macOS, temporary
/// folders lie behind a link.
fn real_tempdir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().canonicalize().unwrap();
    (tmp, real)
}

/// A scanner of the library with its one root at `root`, the owner of its thread, and its store.
fn scanner(root: &Path) -> (Scanner, ScannerThread, Arc<BareStore>) {
    let store = Arc::new(BareStore::default());
    let (scanner, thread) = Scanner::start(store.clone(), library(root)).unwrap();
    (scanner, thread, store)
}

/// The library's scans in `store`.
fn scans(store: &BareStore) -> Vec<Scan> {
    store.scans()
}

/// Debounce and check intervals of 20 ms each, so a test waits only a moment.
fn timing() -> Timing {
    let quick = Duration::from_millis(20);
    Timing { debounce: quick, check: quick }
}

/// Debounce and check intervals of one second each, for tests that give `Roots::check` times.
fn seconds() -> Timing {
    let second = Duration::from_secs(1);
    Timing { debounce: second, check: second }
}

/// A real watch of `root` that leaves nothing out.
fn watched(root: &Path, events: &Events, stop: &Stop) -> RootWatch {
    RootWatch::start(root, &Arc::default(), events, stop)
}

/// The roots of the library with its one root at `root`, watched as `timing` says.
fn roots_at(scanner: &Scanner, root: &Path, timing: &Timing, stop: &Stop) -> Roots {
    let paths = [root.to_path_buf()];
    Roots::start(scanner.clone(), &paths, mpsc::channel().0, Arc::default(), timing, stop)
}

/// The scopes of the scans in `store`.
fn scoped(store: &BareStore) -> Vec<Vec<Scope>> {
    scans(store).into_iter().map(|scan| scan.scopes).collect()
}

/// Replaces the directory at `root` with a new one, as a share mounted over it would.
fn replace(root: &Path, aside: &Path) {
    fs::rename(root, aside).unwrap();
    fs::create_dir(root).unwrap();
}

/// Writes files into `folder`, in `root`, until a watcher scan newer than scan `after` covers
/// just that folder, and returns that scan's id.
fn seen(store: &BareStore, root: &Path, folder: &Path, after: i64) -> i64 {
    let scope = vec![Scope::folder(root, folder, Depth::Directory)];
    let deadline = Instant::now() + PATIENCE;
    let mut n = 0;
    loop {
        fs::write(folder.join(format!("{n}.txt")), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let scans = scans(store);
        if let Some(scan) = scans.iter().find(|s| s.id > after && s.scopes == scope) {
            break scan.id;
        }
        let folder = folder.display();
        assert!(Instant::now() < deadline, "no watcher scan of {folder} after {after}: {scans:?}");
        n += 1;
    }
}

#[test]
fn a_watch_covers_the_whole_root_or_says_why_not() {
    let (_alive, stop) = Stop::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir_all(root.join("a/b")).unwrap();
    let events = mpsc::channel().0;
    let watch = watched(&root, &events, &stop);
    assert!(watch.coverage.is_ok());
    assert_eq!(watch.identity, identity(&root));
    // A local folder reports every change, so nothing warns that it may not.
    assert_eq!(watch.unreported, None);
    let missing = watched(&tmp.path().join("missing"), &events, &stop);
    assert!(matches!(missing.coverage.unwrap_err().kind, ErrorKind::PathNotFound));
    assert_eq!(missing.identity, None);
    assert_eq!(missing.unreported, None);
}

/// A root whose directory is replaced is watched afresh and scanned, since what changed while
/// it was not watched was missed; one still the same directory is left alone.
#[test]
fn a_root_whose_directory_is_replaced_is_watched_afresh_and_scanned() {
    let (_alive, stop) = Stop::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    let (scanner, thread, store) = scanner(&root);
    let mut roots = roots_at(&scanner, &root, &seconds(), &stop);
    let start = Instant::now();
    roots.check(start + Duration::from_millis(1500), &stop);
    assert_eq!(scoped(&store), Vec::<Vec<Scope>>::new(), "an unchanged root was scanned");

    replace(&root, &tmp.path().join("aside"));
    roots.check(start + Duration::from_millis(3000), &stop);
    assert_eq!(scoped(&store), [vec![Scope::root(root.clone())]]);
    assert_eq!(roots.roots[0].watch.identity, identity(&root));
    roots.check(start + Duration::from_millis(4500), &stop);
    assert_eq!(scans(&store).len(), 1, "the root was scanned again as it was");
    drop(roots);
    drop(thread);
}

/// A root that disappears, as an unmounted share does, is watched afresh with no identity, and
/// scanned only once a directory is there again.
#[test]
fn a_root_that_is_gone_is_scanned_once_it_returns() {
    let (_alive, stop) = Stop::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let aside = tmp.path().join("aside");
    fs::create_dir(&root).unwrap();
    let (scanner, thread, store) = scanner(&root);
    let mut roots = roots_at(&scanner, &root, &seconds(), &stop);
    let start = Instant::now();

    fs::rename(&root, &aside).unwrap();
    for second in [2, 4] {
        roots.check(start + Duration::from_secs(second), &stop);
    }
    assert_eq!(roots.roots[0].watch.identity, None);
    assert_eq!(scans(&store), [], "a root that is gone was scanned");

    fs::create_dir(&root).unwrap();
    roots.check(start + Duration::from_secs(6), &stop);
    assert_eq!(scoped(&store), [vec![Scope::root(root.clone())]]);
    drop(roots);
    drop(thread);
}

/// A root found changed is scanned even when its fresh watch covers only part of it, here a
/// file where the directory was, which cannot be listed.
#[test]
fn a_partial_watch_still_queues_the_scan() {
    let (_alive, stop) = Stop::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    let (scanner, thread, store) = scanner(&root);
    let mut roots = roots_at(&scanner, &root, &seconds(), &stop);

    fs::rename(&root, tmp.path().join("aside")).unwrap();
    fs::write(&root, b"x").unwrap();
    roots.check(Instant::now() + Duration::from_secs(2), &stop);
    assert!(roots.roots[0].watch.coverage.is_err(), "the watch covered a file's folders");
    assert_eq!(scoped(&store), [vec![Scope::root(root.clone())]]);
    drop(roots);
    drop(thread);
}

/// Roots are looked at once per check: one replaced within the interval after a check waits for
/// the next.
#[test]
fn roots_are_checked_once_per_interval() {
    let (_alive, stop) = Stop::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    let (scanner, thread, store) = scanner(&root);
    let mut roots = roots_at(&scanner, &root, &seconds(), &stop);
    let start = Instant::now();
    roots.check(start + Duration::from_millis(1500), &stop);
    replace(&root, &tmp.path().join("aside"));
    roots.check(start + Duration::from_millis(2000), &stop);
    assert_eq!(scans(&store), [], "a root was checked twice in one interval");
    roots.check(start + Duration::from_millis(3000), &stop);
    assert_eq!(scans(&store).len(), 1);
    drop(roots);
    drop(thread);
}
