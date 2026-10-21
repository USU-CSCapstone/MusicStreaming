//! The roots as the watcher's own thread keeps them.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::triggers::{FsWatcher, STOP_WAIT};
use crate::types::Depth;

/// The root's watch is set up on the watcher's own thread, so the server listens without
/// waiting for it.
#[test]
fn watching_is_set_up_after_start_returns() {
    let tmp = tempfile::tempdir().unwrap();
    let (scanner, _) = scanner(tmp.path());
    let (release, released) = mpsc::channel::<()>();
    let (entered, set_up) = mpsc::channel();
    let waiting = move |root: &Path, events: &Events| {
        entered.send(released.recv_timeout(PATIENCE).is_ok()).unwrap();
        RootWatch::start(root, events)
    };
    let watcher = FsWatcher::spawn(scanner.clone(), vec![tmp.path().into()], waiting, timing(None));
    release.send(()).unwrap();
    assert!(set_up.recv_timeout(2 * PATIENCE).unwrap(), "start waited for the watch");
    watcher.stop();
    scanner.shutdown();
}

/// A watch still being set up, as of a large root, does not hold up the shutdown.
#[test]
fn stopping_does_not_wait_for_a_watch_being_set_up() {
    let tmp = tempfile::tempdir().unwrap();
    let (scanner, _) = scanner(tmp.path());
    let (release, released) = mpsc::channel::<()>();
    let held = move |root: &Path, events: &Events| {
        assert!(released.recv_timeout(PATIENCE).is_ok(), "never released");
        RootWatch::start(root, events)
    };
    let watcher = FsWatcher::spawn(scanner.clone(), vec![tmp.path().into()], held, timing(None));
    let stopping = Instant::now();
    watcher.stop();
    assert!(stopping.elapsed() < 2 * STOP_WAIT, "stopping took {:?}", stopping.elapsed());
    release.send(()).unwrap();
    scanner.shutdown();
}

/// Stopping an idle watcher waits for its thread to end, so what the thread holds, such as the
/// scanner and through it the database, is let go before the process ends.
#[test]
fn stopping_an_idle_watcher_waits_for_its_thread() {
    let tmp = tempfile::tempdir().unwrap();
    let (scanner, _) = scanner(tmp.path());
    let watches = Arc::new(AtomicUsize::new(0));
    let held = watches.clone();
    let counted = move |root: &Path, events: &Events| {
        held.fetch_add(1, Ordering::SeqCst);
        RootWatch::start(root, events)
    };
    let watcher = FsWatcher::spawn(scanner.clone(), vec![tmp.path().into()], counted, timing(None));
    let deadline = Instant::now() + PATIENCE;
    while watches.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the watch was never set up");
        std::thread::sleep(Duration::from_millis(10));
    }
    watcher.stop();
    assert_eq!(Arc::strong_count(&watches), 1, "the watcher's thread outlived the stop");
    scanner.shutdown();
}

/// The watcher meets the OS limit on watches as it watches a folder created in the root: the
/// library then falls back to its schedule, with no rescan.
#[test]
fn the_limit_met_while_watching_brings_the_fallback_not_a_rescan() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (scanner, store) = scanner(&root);
    let limit_met = |root: &Path, events: &Events| {
        let error = notify::Error::new(ErrorKind::MaxFilesWatch).add_path(root.join("new"));
        events.send(Err(error)).unwrap();
        RootWatch::start(root, events)
    };
    let fallback = Some(Duration::from_millis(20));
    let watcher = FsWatcher::spawn(scanner.clone(), vec![root], limit_met, timing(fallback));
    let deadline = Instant::now() + PATIENCE;
    while !scans(&store).iter().any(|scan| scan.trigger == Trigger::Scheduled) {
        assert!(Instant::now() < deadline, "no fallback scan: {:?}", scans(&store));
        std::thread::sleep(Duration::from_millis(20));
    }
    watcher.stop();
    scanner.shutdown();
    assert!(!scans(&store).iter().any(|scan| scan.trigger == Trigger::Watch));
}

/// Another directory takes the root's place, as a share mounted over it would, and its
/// changes are seen.
#[test]
fn a_root_replaced_under_its_path_is_watched_afresh() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    let (scanner, store) = scanner(&root);
    let watcher =
        FsWatcher::spawn(scanner.clone(), vec![root.clone()], RootWatch::start, timing(None));
    let folder = vec![Scope::folder(&root, &root, Depth::Directory)];
    // Writes into the root until a watcher scan newer than `after` covers just that write.
    let seen = |after: u64| {
        let deadline = Instant::now() + PATIENCE;
        let mut n = 0;
        loop {
            fs::write(root.join(format!("{n}.txt")), b"x").unwrap();
            std::thread::sleep(Duration::from_millis(50));
            let scans = scans(&store);
            if let Some(scan) = scans.iter().find(|s| s.id > after && s.scopes == folder) {
                break scan.id;
            }
            assert!(Instant::now() < deadline, "no watcher scan after {after}: {scans:?}");
            n += 1;
        }
    };
    let before = seen(0);
    fs::rename(&root, tmp.path().join("old")).unwrap();
    fs::create_dir(&root).unwrap();
    seen(before);
    watcher.stop();
    scanner.shutdown();
}
