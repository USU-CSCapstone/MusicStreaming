//! A scan suspended by an unavailable root is requested again after a fixed wait, and one
//! suspended for any other reason is not (`design/scanning.md` §9).

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use super::super::*;
use crate::scan::ScanState;
use crate::store::StoreError;
use crate::testing::{BareStore, Hooked, library, make_album, wait_until};

/// How long the retry waits in the test that checks the wait.
const WAIT: Duration = Duration::from_millis(600);

/// A library of two roots, `nas` and `kept`, with one album each, where `nas` has gone, as an
/// unmounted share does, and the path it has gone to.
fn nas_gone(tmp: &Path) -> (LibraryConfig, PathBuf, PathBuf) {
    let (nas, kept, away) = (tmp.join("1-nas"), tmp.join("2-kept"), tmp.join("away"));
    make_album(&kept, "A", "X", 1);
    make_album(&nas, "B", "Y", 1);
    fs::rename(&nas, &away).unwrap();
    (LibraryConfig { roots: vec![nas.clone(), kept.clone()], ..library(&kept) }, nas, away)
}

/// A scan that suspends because a root has gone is requested again with its trigger, for the
/// scopes of the roots that did not answer, once the wait has passed and not before, and once the
/// root is back, the retry indexes it.
#[test]
fn a_root_that_did_not_answer_is_retried_after_the_wait() {
    let tmp = tempfile::tempdir().unwrap();
    let (lib, nas, away) = nas_gone(tmp.path());
    let store = Arc::new(BareStore::default());
    let options = ScanOptions { retry_wait: WAIT, ..Default::default() };
    let (scanner, thread) = Scanner::start_with(store.clone(), lib, options).unwrap();
    let suspended_at = Instant::now();
    let first = scanner.scan_library(Trigger::Initial).unwrap();
    scanner.wait_idle();
    assert_eq!(store.scans().len(), 1, "the retry came before the wait had passed");

    fs::rename(&away, &nas).unwrap();
    let ended = || store.scans().get(1).is_some_and(|s| s.finished_at.is_some());
    wait_until("the retry never ended", ended);
    assert!(suspended_at.elapsed() >= WAIT, "the retry came before the wait had passed");
    drop(thread);
    let [suspended, retry] = store.scans().try_into().unwrap();
    assert_eq!((suspended.id, suspended.state), (first, ScanState::Suspended));
    assert_eq!((retry.trigger, retry.state), (Trigger::Initial, ScanState::Completed));
    assert_eq!(retry.scopes, [Scope::root(&nas)], "only the root that did not answer");
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
    let options = ScanOptions { retry_wait: Duration::ZERO, ..Default::default() };
    let (scanner, thread) = Scanner::start_with(store.clone(), lib, options).unwrap();
    scanner.scan_library(Trigger::Manual).unwrap();
    scanner.wait_idle();

    thread::sleep(Duration::from_millis(200));
    drop(thread);
    let [row] = store.scans().try_into().unwrap();
    assert_eq!(row.state, ScanState::Suspended);
}

/// A scan suspended by a store failure is not retried.
#[test]
fn a_store_failure_is_not_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let store = Arc::new(BareStore::default());
    let failing = Hooked::new(store.clone()).on_apply(|_| Err(StoreError("disk full".into())));
    let options = ScanOptions { retry_wait: Duration::ZERO, ..Default::default() };
    let (scanner, thread) =
        Scanner::start_with(Arc::new(failing), library(&root), options).unwrap();
    scanner.scan_library(Trigger::Manual).unwrap();
    scanner.wait_idle();

    thread::sleep(Duration::from_millis(200));
    drop(thread);
    let [row] = store.scans().try_into().unwrap();
    assert_eq!(row.state, ScanState::Suspended);
}
