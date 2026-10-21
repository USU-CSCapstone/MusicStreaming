//! The roots as the watcher's own thread keeps them.

use std::sync::atomic::{AtomicUsize, Ordering};

use notify::EventKind;
use notify::event::Flag;

use super::*;
use crate::library::LibraryConfig;
use crate::triggers::{FsWatcher, STOP_WAIT};

/// The root's watch is set up on the watcher's own thread, so the server listens without
/// waiting for it.
#[test]
fn watching_is_set_up_after_start_returns() {
    let tmp = tempfile::tempdir().unwrap();
    let (scanner, thread, _) = scanner(tmp.path());
    let (release, released) = mpsc::channel::<()>();
    let (entered, set_up) = mpsc::channel();
    let waiting = move |root: &Path, events: &Events| {
        entered.send(released.recv_timeout(PATIENCE).is_ok()).unwrap();
        watched(root, events)
    };
    let watcher =
        FsWatcher::spawn(scanner.clone(), vec![tmp.path().into()], waiting, timing(None)).unwrap();
    release.send(()).unwrap();
    assert!(set_up.recv_timeout(2 * PATIENCE).unwrap(), "start waited for the watch");
    drop(watcher);
    drop(thread);
}

/// A watch still being set up, as of a large root, does not hold up the shutdown.
#[test]
fn stopping_does_not_wait_for_a_watch_being_set_up() {
    let tmp = tempfile::tempdir().unwrap();
    let (scanner, thread, _) = scanner(tmp.path());
    let (release, released) = mpsc::channel::<()>();
    let held = move |root: &Path, events: &Events| {
        assert!(released.recv_timeout(PATIENCE).is_ok(), "never released");
        watched(root, events)
    };
    let watcher =
        FsWatcher::spawn(scanner.clone(), vec![tmp.path().into()], held, timing(None)).unwrap();
    let stopping = Instant::now();
    drop(watcher);
    assert!(stopping.elapsed() < 2 * STOP_WAIT, "stopping took {:?}", stopping.elapsed());
    release.send(()).unwrap();
    drop(thread);
}

/// Stopping an idle watcher waits for its thread to end, so what the thread holds, such as the
/// scanner and through it the database, is let go before the process ends.
#[test]
fn stopping_an_idle_watcher_waits_for_its_thread() {
    let tmp = tempfile::tempdir().unwrap();
    let (scanner, thread, _) = scanner(tmp.path());
    let watches = Arc::new(AtomicUsize::new(0));
    let held = watches.clone();
    let counted = move |root: &Path, events: &Events| {
        held.fetch_add(1, Ordering::SeqCst);
        watched(root, events)
    };
    let watcher =
        FsWatcher::spawn(scanner.clone(), vec![tmp.path().into()], counted, timing(None)).unwrap();
    let deadline = Instant::now() + PATIENCE;
    while watches.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the watch was never set up");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(watcher);
    assert_eq!(Arc::strong_count(&watches), 1, "the watcher's thread outlived the stop");
    drop(thread);
}

/// The watcher meets the OS limit on watches as it watches a folder made in the root: the
/// library then falls back to its schedule, with no rescan.
#[test]
fn the_limit_met_while_watching_brings_the_fallback_not_a_rescan() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (scanner, thread, store) = scanner(&root);
    let limit_at_new =
        |root: &Path, events: &Events| FailsAt::watch(root, events, root.join("new"), limit_error);
    let fallback = Some(Duration::from_millis(20));
    let watcher =
        FsWatcher::spawn(scanner.clone(), vec![root.clone()], limit_at_new, timing(fallback))
            .unwrap();
    // Waits until the root is watched, so the new folder's event is seen.
    seen(&store, &root, &root, 0);
    fs::create_dir(root.join("new")).unwrap();
    let deadline = Instant::now() + PATIENCE;
    while !scans(&store).iter().any(|scan| scan.trigger == Trigger::Scheduled) {
        assert!(Instant::now() < deadline, "no fallback scan: {:?}", scans(&store));
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(watcher);
    drop(thread);
    let library = Scope::root(root);
    let rescan = |scan: &Scan| scan.trigger == Trigger::Watch && scan.scopes.contains(&library);
    assert!(!scans(&store).iter().any(rescan), "{:?}", scans(&store));
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
        FsWatcher::spawn(scanner.clone(), vec![root.clone()], watched, timing(None)).unwrap();
    let before = seen(&store, &root, &root, 0);
    fs::rename(&root, tmp.path().join("old")).unwrap();
    fs::create_dir(&root).unwrap();
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
    let store = Arc::new(MemoryStore::new());
    let library = LibraryConfig { excludes: vec!["skip".into()], ..library(&root) };
    let (scanner, thread) = Scanner::start(store.clone(), library).unwrap();
    let debounce = Duration::from_millis(20);
    let watcher =
        FsWatcher::start(scanner.clone(), slice::from_ref(&root), debounce, None).unwrap();
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
    let failing = |root: &Path, events: &Events| {
        events.send(Err(notify::Error::generic("lost"))).unwrap();
        watched(root, events)
    };
    let watcher =
        FsWatcher::spawn(scanner.clone(), vec![root.clone()], failing, timing(None)).unwrap();
    let library = Scope::root(root);
    let rescan = |scan: &Scan| scan.trigger == Trigger::Watch && scan.scopes.contains(&library);
    let deadline = Instant::now() + PATIENCE;
    while !scans(&store).iter().any(rescan) {
        assert!(Instant::now() < deadline, "no rescan: {:?}", scans(&store));
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(watcher);
    drop(thread);
}

/// Once the OS drops events, every root is walked again, so a folder made meanwhile, whose
/// event was dropped, is watched.
#[test]
fn dropped_events_have_every_root_walked_again() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    fs::create_dir(root.join("a")).unwrap();
    let (scanner, thread, _) = scanner(&root);
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let overflowing = move |root: &Path, events: &Events| {
        let watch = FailsAt::counted(root, events, &counted);
        let dropped = notify::Event::new(EventKind::Other).set_flag(Flag::Rescan);
        events.send(Ok(dropped)).unwrap();
        watch
    };
    let watcher = FsWatcher::spawn(scanner.clone(), vec![root], overflowing, timing(None)).unwrap();
    // The root and `a`, as the watch starts and again.
    let deadline = Instant::now() + PATIENCE;
    while calls.load(Ordering::SeqCst) < 4 {
        assert!(Instant::now() < deadline, "{} folders watched", calls.load(Ordering::SeqCst));
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(watcher);
    drop(thread);
}

/// With an exclude pattern that does not compile, every folder is watched.
#[test]
fn every_folder_is_watched_when_an_exclude_does_not_compile() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let sub = root.join("sub");
    fs::create_dir(&sub).unwrap();
    let store = Arc::new(MemoryStore::new());
    let library = LibraryConfig { excludes: vec!["[".into()], ..library(&root) };
    let (scanner, thread) = Scanner::start(store.clone(), library).unwrap();
    let debounce = Duration::from_millis(20);
    let watcher =
        FsWatcher::start(scanner.clone(), slice::from_ref(&root), debounce, None).unwrap();
    seen(&store, &root, &sub, 0);
    drop(watcher);
    drop(thread);
}
