//! The roots as the watcher's own thread keeps them.

use super::*;
use crate::library::LibraryConfig;
use crate::triggers::{debounce_loop, spawn_watch, watch};
use crate::worker::Worker;

/// Runs the watcher's loop over `roots` and `rx` on a worker, which stops when dropped.
fn looping(
    scanner: &Scanner,
    roots: Roots,
    rx: mpsc::Receiver<notify::Result<notify::Event>>,
) -> Worker {
    let scanner = scanner.clone();
    Worker::spawn("test-watch", move |stop| {
        debounce_loop(scanner, roots, rx, &stop, Duration::from_millis(20));
    })
    .unwrap()
}

/// Stopping an idle watcher waits for its thread to end, so what the thread holds, such as the
/// scanner and through it the database, is let go before the process ends.
#[test]
fn stopping_an_idle_watcher_waits_for_its_thread() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (scanner, thread, store) = scanner(&root);
    let watcher =
        spawn_watch(scanner.clone(), vec![root.clone()], Arc::default(), timing()).unwrap();
    // The watcher has started once it sees a change.
    seen(&store, &root, &root, 0);
    drop(watcher);
    drop(thread);
    drop(scanner);
    assert_eq!(Arc::strong_count(&store), 1, "the watcher's thread outlived the stop");
}

/// Another directory takes the root's place, as a share mounted over it would, and its
/// changes are seen.
#[test]
fn a_root_replaced_under_its_path_is_watched_afresh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    let (scanner, thread, store) = scanner(&root);
    let watcher =
        spawn_watch(scanner.clone(), vec![root.clone()], Arc::default(), timing()).unwrap();
    let before = seen(&store, &root, &root, 0);
    replace(&root, &tmp.path().join("old"));
    seen(&store, &root, &root, before);
    drop(watcher);
    drop(thread);
}

/// The watcher leaves out what the library's excludes match.
#[test]
fn what_the_library_excludes_is_not_watched() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let skip = root.join("skip");
    fs::create_dir(&skip).unwrap();
    let store = Arc::new(BareStore::default());
    let library = LibraryConfig { excludes: vec!["skip".into()], ..library(&root) };
    let (scanner, thread) = Scanner::start(store.clone(), library).unwrap();
    let debounce = Duration::from_millis(20);
    let watcher = watch(scanner.clone(), slice::from_ref(&root), debounce).unwrap();
    let watching = seen(&store, &root, &root, 0);
    // A watch delivers events in order, so a scope for this write, were `skip` watched, would be
    // queued by the time the root's next write is scanned.
    fs::write(skip.join("01.flac"), b"x").unwrap();
    seen(&store, &root, &root, watching);
    drop(watcher);
    drop(thread);
    let scopes: Vec<Scope> = scans(&store).into_iter().flat_map(|scan| scan.scopes).collect();
    assert!(!scopes.iter().any(|scope| scope.path.starts_with(&skip)), "{scopes:?}");
}

/// An error from a watch rescans the library, since what it missed is unknown.
#[test]
fn a_watch_error_rescans_the_library() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (scanner, thread, store) = scanner(&root);
    let (tx, rx) = mpsc::channel();
    let (_alive, stop) = Stop::new();
    let roots = Roots::start(
        scanner.clone(),
        slice::from_ref(&root),
        tx.clone(),
        Arc::default(),
        &timing(),
        &stop,
    );
    let looping = looping(&scanner, roots, rx);
    tx.send(Err(notify::Error::generic("lost"))).unwrap();
    let library = Scope::root(root);
    let rescan = |scan: &Scan| scan.trigger == Trigger::Watch && scan.scopes.contains(&library);
    let deadline = Instant::now() + PATIENCE;
    while !scans(&store).iter().any(rescan) {
        assert!(Instant::now() < deadline, "no rescan: {:?}", scans(&store));
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(looping);
    drop(thread);
}
