use std::cell::Cell;
use std::slice;
use std::sync::{Arc, mpsc};

use std::collections::BTreeSet;

use super::*;
use crate::Governor;
use crate::scan::ScanOptions;
use crate::store::MemoryStore;
use crate::testing::library;
use crate::types::Scan;
use fakes::{FailsAt, limit_error, seen};

mod coverage;
mod fakes;
mod watcher;

/// How long a test waits for what it expects before it fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// A scanner of the library with its one root at `root`, and its store.
fn scanner(root: &Path) -> (Scanner, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::new());
    let governor = Arc::new(Governor::new());
    let scanner = Scanner::start(store.clone(), library(root), governor, ScanOptions::default());
    (scanner, store)
}

/// The library's scans in `store`.
fn scans(store: &MemoryStore) -> Vec<Scan> {
    store.scans(1)
}

/// Short waits throughout, and a fallback schedule of `fallback`.
fn timing(fallback: Option<Duration>) -> Timing {
    let quick = Duration::from_millis(20);
    Timing { debounce: quick, check: quick, fallback }
}

/// Checks once a second, and scans every hour while the library is not wholly watched.
fn seconds() -> Timing {
    let second = Duration::from_secs(1);
    Timing { debounce: second, check: second, fallback: Some(MAX_BACKOFF) }
}

/// A watch with no watcher, of the directory `identity`, covering what `coverage` says.
fn fake(identity: Option<(u64, u64)>, coverage: notify::Result<()>) -> RootWatch {
    let (excludes, unreadable) = (Arc::default(), BTreeSet::new());
    RootWatch { watcher: None, identity, coverage, excludes, unreadable }
}

/// A real watch of `root` that leaves nothing out.
fn watched(root: &Path, events: &Events) -> RootWatch {
    RootWatch::start(root, &Arc::default(), events)
}

/// The watch of a root stopped by the OS limit on watches before it covered anything.
fn limited(root: &Path, _: &Events) -> RootWatch {
    fake(identity(root), Err(notify::Error::new(ErrorKind::MaxFilesWatch)))
}

/// A watch of some other directory, so every check finds the root lost.
fn elsewhere(_: &Path, _: &Events) -> RootWatch {
    fake(Some((0, 0)), Ok(()))
}

/// The backoff of the one root in `roots`.
fn backoff<W>(roots: &Roots<W>) -> Duration {
    roots.roots[0].backoff
}

#[test]
fn a_watch_covers_the_whole_root_or_says_why_not() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir_all(root.join("a/b")).unwrap();
    let events = mpsc::channel().0;
    let watch = watched(&root, &events);
    assert!(watch.coverage.is_ok());
    assert_eq!(watch.identity, identity(&root));
    let missing = watched(&tmp.path().join("missing"), &events);
    assert!(matches!(missing.coverage.unwrap_err().kind, ErrorKind::PathNotFound));
    assert_eq!(missing.identity, None);
}

#[test]
fn a_library_not_wholly_watched_is_scanned_on_the_fallback_schedule() {
    let tmp = tempfile::tempdir().unwrap();
    let root = vec![tmp.path().to_path_buf()];
    let (scanner, store) = scanner(tmp.path());
    let start = |watch: fn(&Path, &Events) -> RootWatch, fallback| {
        Roots::start(scanner.clone(), &root, mpsc::channel().0, watch, &timing(fallback))
    };
    let every = Some(Duration::from_millis(20));
    let whole = start(watched, every);
    assert!(whole.fallback.is_none(), "a library wholly watched needs no fallback");
    whole.stop();
    let scheduled = start(limited, None);
    assert!(scheduled.fallback.is_none(), "a library with a schedule keeps to it");
    scheduled.stop();

    let partial = start(limited, every);
    let deadline = Instant::now() + PATIENCE;
    while !scans(&store).iter().any(|scan| scan.trigger == Trigger::Scheduled) {
        assert!(Instant::now() < deadline, "no fallback scan");
        std::thread::sleep(Duration::from_millis(20));
    }
    partial.stop();
    // Stopping ends the fallback schedule, which would otherwise fire within 250 ms.
    scanner.wait_idle();
    let stopped = scans(&store).len();
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(scans(&store).len(), stopped, "the fallback schedule outlived the stop");
    scanner.shutdown();
}

#[test]
fn a_root_watched_afresh_is_scanned_and_one_whose_watch_fails_is_not() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (scanner, store) = scanner(&root);
    let roots_at = slice::from_ref(&root);
    let start = Instant::now();
    let mut roots =
        Roots::start(scanner.clone(), roots_at, mpsc::channel().0, elsewhere, &seconds());
    assert_eq!(scans(&store), []);
    roots.check(start + Duration::from_millis(1500));
    let scopes: Vec<Vec<Scope>> = scans(&store).into_iter().map(|scan| scan.scopes).collect();
    assert_eq!(scopes, [vec![Scope::root(root.clone())]]);
    roots.stop();
    // So a later request cannot join that scan.
    scanner.wait_idle();

    let tries = Cell::new(0);
    // Lost at once, then fails each time it is watched afresh.
    let failing = |_: &Path, _: &Events| {
        tries.set(tries.get() + 1);
        let coverage = if tries.get() == 1 { Ok(()) } else { Err(notify::Error::generic("no")) };
        fake(Some((0, 0)), coverage)
    };
    let mut roots = Roots::start(scanner.clone(), roots_at, mpsc::channel().0, failing, &seconds());
    let scanned = scans(&store).len();
    roots.check(start + Duration::from_millis(1500));
    assert_eq!(tries.get(), 2);
    assert_eq!(scans(&store).len(), scanned, "a failed watch was scanned");
    roots.stop();
    scanner.shutdown();
}

/// A watch that fails is tried again after a wait that doubles each time, until it covers the
/// root, which ends the fallback schedule; once the root is found as watched, the wait is
/// short again. One stopped by the OS limit on watches is not tried again.
#[test]
fn a_failed_watch_is_tried_again_after_a_growing_wait() {
    let tmp = tempfile::tempdir().unwrap();
    let root = vec![tmp.path().to_path_buf()];
    let (scanner, _) = scanner(tmp.path());
    let tries = Cell::new(0);
    // Fails twice, then covers the root.
    let flaky = |root: &Path, _: &Events| {
        tries.set(tries.get() + 1);
        let coverage = if tries.get() < 3 { Err(notify::Error::generic("flaky")) } else { Ok(()) };
        fake(identity(root), coverage)
    };
    let start = Instant::now();
    let mut roots = Roots::start(scanner.clone(), &root, mpsc::channel().0, flaky, &seconds());
    assert!(roots.fallback.is_some());
    for (after, expected) in [(1500, 2), (3000, 2), (4000, 3), (9000, 3)] {
        roots.check(start + Duration::from_millis(after));
        assert_eq!(tries.get(), expected, "{after} ms after the start");
    }
    assert!(roots.fallback.is_none(), "the fallback schedule outlived the failure");
    assert_eq!(backoff(&roots), Duration::from_secs(8), "the wait was reset too soon");
    roots.check(start + Duration::from_secs(12));
    assert_eq!(backoff(&roots), seconds().check, "the wait outlived the failure");
    roots.stop();

    let limited_tries = Cell::new(0);
    let counted = |root: &Path, events: &Events| {
        limited_tries.set(limited_tries.get() + 1);
        limited(root, events)
    };
    let mut roots = Roots::start(scanner.clone(), &root, mpsc::channel().0, counted, &seconds());
    roots.check(start + 2 * MAX_BACKOFF);
    assert_eq!(limited_tries.get(), 1, "a watch the OS limit stopped was tried again");
    roots.stop();
    scanner.shutdown();
}

/// A fresh watch that meets the OS limit, as it may while the watches it replaced are let go,
/// is tried once more, and only once.
#[test]
fn a_fresh_watch_that_meets_the_limit_is_tried_once_more() {
    let tmp = tempfile::tempdir().unwrap();
    let root = vec![tmp.path().to_path_buf()];
    let (scanner, _) = scanner(tmp.path());
    let tries = Cell::new(0);
    // Lost at once, then each fresh watch meets the limit.
    let limited_afresh = |root: &Path, events: &Events| {
        tries.set(tries.get() + 1);
        if tries.get() == 1 { elsewhere(root, events) } else { limited(root, events) }
    };
    let start = Instant::now();
    let mut roots =
        Roots::start(scanner.clone(), &root, mpsc::channel().0, limited_afresh, &seconds());
    for after in [1500, 4000] {
        roots.check(start + Duration::from_millis(after));
    }
    assert_eq!(tries.get(), 3, "the fresh watch was not tried once more");
    for n in 1..=5 {
        roots.check(start + n * 2 * MAX_BACKOFF);
    }
    assert_eq!(tries.get(), 3, "the limit was tried more than once more");
    roots.stop();
    scanner.shutdown();
}

#[test]
fn the_wait_between_tries_stops_growing_at_its_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let root = vec![tmp.path().to_path_buf()];
    let (scanner, _) = scanner(tmp.path());
    let failing = |root: &Path, _: &Events| fake(identity(root), Err(notify::Error::generic("no")));
    let start = Instant::now();
    let mut roots = Roots::start(scanner.clone(), &root, mpsc::channel().0, failing, &seconds());
    for n in 1..=20 {
        roots.check(start + n * 2 * MAX_BACKOFF);
    }
    assert_eq!(backoff(&roots), MAX_BACKOFF);
    roots.stop();
    scanner.shutdown();
}

/// A root whose directory keeps changing, as a flapping mount would, is watched afresh and
/// scanned less and less often, not at every check.
#[test]
fn a_flapping_root_is_scanned_less_and_less_often() {
    let tmp = tempfile::tempdir().unwrap();
    let root = vec![tmp.path().to_path_buf()];
    let (scanner, store) = scanner(tmp.path());
    let tries = Cell::new(0);
    let counted = |root: &Path, events: &Events| {
        tries.set(tries.get() + 1);
        elsewhere(root, events)
    };
    let start = Instant::now();
    let mut roots = Roots::start(scanner.clone(), &root, mpsc::channel().0, counted, &seconds());
    for second in 1..=60 {
        roots.check(start + Duration::from_millis(second * 1000 + 500));
    }
    // Watched at the start, then 1.5, 3.5, 7.5, 15.5 and 31.5 seconds after it.
    assert_eq!(tries.get(), 6);
    assert!(scans(&store).len() <= 5, "{} scans", scans(&store).len());
    roots.stop();
    scanner.shutdown();
}

/// Replaces the directory at `root` with a new one, as a share mounted over it would.
fn replace(root: &Path, aside: &Path) {
    fs::rename(root, aside).unwrap();
    fs::create_dir(root).unwrap();
}

/// Roots are looked at once per check: one lost within the interval after a check waits for
/// the next.
#[test]
fn roots_are_checked_once_per_interval() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    let (scanner, _) = scanner(&root);
    let tries = Cell::new(0);
    let counted = |root: &Path, _: &Events| {
        tries.set(tries.get() + 1);
        fake(identity(root), Ok(()))
    };
    let start = Instant::now();
    let roots_at = slice::from_ref(&root);
    let mut roots = Roots::start(scanner.clone(), roots_at, mpsc::channel().0, counted, &seconds());
    roots.check(start + Duration::from_millis(1500));
    replace(&root, &tmp.path().join("aside"));
    roots.check(start + Duration::from_millis(2000));
    assert_eq!(tries.get(), 1, "a root was checked twice in one interval");
    roots.check(start + Duration::from_millis(2500));
    assert_eq!(tries.get(), 2);
    roots.stop();
    scanner.shutdown();
}

/// A mount that comes and goes every other check is watched afresh, and scanned, only a few
/// times: one check that finds it as watched does not cut the wait short.
#[test]
fn a_root_that_comes_and_goes_is_scanned_only_a_few_times() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let (mounted, unmounted) = (tmp.path().join("mounted"), tmp.path().join("unmounted"));
    fs::create_dir(&root).unwrap();
    fs::create_dir(&unmounted).unwrap();
    let (scanner, store) = scanner(&root);
    let tries = Cell::new(0);
    let counted = |root: &Path, _: &Events| {
        tries.set(tries.get() + 1);
        fake(identity(root), Ok(()))
    };
    let start = Instant::now();
    let roots_at = slice::from_ref(&root);
    let mut roots = Roots::start(scanner.clone(), roots_at, mpsc::channel().0, counted, &seconds());
    for check in 1..=60 {
        if check % 2 == 0 {
            // Swaps the two directories, so the root alternates between them.
            let (out, back) =
                if check % 4 == 0 { (&unmounted, &mounted) } else { (&mounted, &unmounted) };
            fs::rename(&root, out).unwrap();
            fs::rename(back, &root).unwrap();
        }
        roots.check(start + Duration::from_millis(check * 1000 + 500));
    }
    assert!(tries.get() <= 6, "watched {} times", tries.get());
    roots.stop();
    scanner.wait_idle();
    assert!(scans(&store).len() <= 5, "{} scans", scans(&store).len());
    scanner.shutdown();
}
