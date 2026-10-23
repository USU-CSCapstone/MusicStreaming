//! A suspended scan's work is queued again after a wait, unless what suspended it would suspend
//! the retry too (`design/scanning.md` §9). [`Scanner::retry_in`] shortens the wait, whose
//! length `retry`'s own tests check, and the worker queues the work once it has passed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::super::*;
use crate::scan::ScanState;
use crate::testing::{BareStore, library, make_album, wait_until};

/// A library of two roots, `nas` and `kept`, with one album each, where `nas` has gone, as an
/// unmounted share does, and the path it has gone to. `nas` sorts first, so a scan has walked
/// nothing when it finds it gone.
fn nas_gone(tmp: &Path) -> (LibraryConfig, PathBuf, PathBuf) {
    let (nas, kept, away) = (tmp.join("1-nas"), tmp.join("2-kept"), tmp.join("away"));
    make_album(&kept, "A", "X", 1);
    make_album(&nas, "B", "Y", 1);
    fs::rename(&nas, &away).unwrap();
    (LibraryConfig { roots: vec![nas.clone(), kept.clone()], ..library(&kept) }, nas, away)
}

/// A scanner of `lib` over `store`, and the owner of its thread.
fn scanner(store: &Arc<BareStore>, lib: &LibraryConfig) -> (Scanner, ScannerThread) {
    Scanner::start(store.clone(), lib.clone()).unwrap()
}

/// Shortens the wait of the work `scanner` holds to retry to `wait`, and waits for the scan the
/// worker queues for it once the wait has passed to end, as the second of `store`'s scans.
fn retried(scanner: &Scanner, store: &BareStore, wait: Duration) {
    assert!(scanner.retry_in(wait), "nothing waits to be retried");
    let ended = || store.scans().get(1).is_some_and(|s| s.finished_at.is_some());
    wait_until("the retry never ended", ended);
}

/// A library scan that suspends because a root has gone leaves that root alone to retry, with
/// the scan's trigger, and once the root is back, the retry indexes it. The worker queues the
/// retry when its own timer says the wait has passed, with nothing else to wake it.
#[test]
fn a_root_that_did_not_answer_is_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let (lib, nas, away) = nas_gone(tmp.path());
    let store = Arc::new(BareStore::default());
    let (scanner, thread) = scanner(&store, &lib);
    let suspended = scanner.scan_library(Trigger::Initial).unwrap();
    scanner.wait_idle();

    fs::rename(&away, &nas).unwrap();
    retried(&scanner, &store, Duration::from_millis(100));
    drop(thread);
    let [first, retry] = store.scans().try_into().unwrap();
    assert_eq!((first.id, first.state), (suspended, ScanState::Suspended));
    assert_eq!((retry.trigger, retry.state), (Trigger::Initial, ScanState::Completed));
    assert_eq!(retry.scopes, [Scope::root(&nas)]);
    assert_eq!(retry.progress.added, 1, "the retry indexed the root: {:?}", retry.progress);
}

/// A scan suspended by an exclude pattern that does not compile is not retried.
#[test]
fn a_pattern_that_does_not_compile_is_not_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let lib = LibraryConfig { excludes: vec!["[".into()], ..library(&root) };
    let store = Arc::new(BareStore::default());
    let (scanner, thread) = scanner(&store, &lib);
    scanner.scan_library(Trigger::Manual).unwrap();
    scanner.wait_idle();

    assert!(!scanner.retry_in(Duration::ZERO));
    drop(thread);
    let [row] = store.scans().try_into().unwrap();
    assert_eq!(row.state, ScanState::Suspended);
}

/// A scan that starts covering the work waiting to be retried does that work, so the work no
/// longer waits and is not walked a second time.
#[test]
fn a_scan_covering_the_waiting_work_takes_it_over() {
    let tmp = tempfile::tempdir().unwrap();
    let (lib, nas, away) = nas_gone(tmp.path());
    let store = Arc::new(BareStore::default());
    let (scanner, thread) = scanner(&store, &lib);
    scanner.scan_library(Trigger::Initial).unwrap();
    scanner.wait_idle();

    fs::rename(&away, &nas).unwrap();
    let scheduled = scanner.scan_library(Trigger::Scheduled).unwrap();
    scanner.wait_idle();
    assert!(!scanner.retry_in(Duration::ZERO));
    drop(thread);
    let row = store.scans().into_iter().find(|s| s.id == scheduled).unwrap();
    assert_eq!(row.state, ScanState::Completed);
}

/// Work waiting to be retried in a root a reconfiguration has removed since is not queued when
/// its wait has passed, nor is an empty scan in its place. The worker takes the work with the
/// queue empty; the library scan queued once it has shows the worker has finished with it, since
/// the worker queues a retry before taking the next scan.
#[test]
fn a_root_removed_from_the_library_is_not_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let (lib, nas, _) = nas_gone(tmp.path());
    let store = Arc::new(BareStore::default());
    let (scanner, thread) = scanner(&store, &lib);
    scanner.scan_library(Trigger::Initial).unwrap();
    scanner.wait_idle();

    let roots = lib.roots.iter().filter(|root| **root != nas).cloned().collect();
    scanner.reconfigure(LibraryConfig { roots, ..lib.clone() }).unwrap();
    scanner.wait_idle();
    assert!(scanner.retry_in(Duration::ZERO), "the root no longer waits");
    wait_until("the worker never took the work", || !scanner.retry_in(Duration::ZERO));
    scanner.scan_library(Trigger::Manual).unwrap();
    scanner.wait_idle();
    drop(thread);
    let rows = store.scans();
    let in_nas = |scan: &Scan| scan.scopes.iter().any(|scope| scope.root == nas);
    assert_eq!(rows.iter().filter(|scan| in_nas(scan)).count(), 1, "{rows:?}");
    assert_eq!(rows.len(), 3, "{rows:?}");
}
