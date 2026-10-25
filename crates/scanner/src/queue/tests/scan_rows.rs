//! Scan rows the queue writes under its lock: a request that joins a queued scan writes none,
//! requests at once for the same work queue it once, and a request the store cannot record is
//! dropped (`design/scanning.md` §9).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use super::super::*;
use crate::scan::ScanState;
use crate::store::StoreError;
use crate::testing::{BareStore, Hooked, library, make_album, wait_until};

/// A scanner of the library at `root`, A/X and B/Y with one track each, over `store`, running a
/// scan of A that `store` holds once its row, one of `rows`, says it runs, with a scan of B
/// queued behind it. Returns the scanner, the owner of its thread, and the two scans' ids.
fn parked(
    root: &Path,
    store: &Arc<Hooked<'static>>,
    rows: &BareStore,
) -> (Scanner, ScannerThread, ScanId, ScanId) {
    make_album(root, "A", "X", 1);
    make_album(root, "B", "Y", 1);
    store.hold();
    let (scanner, thread) = Scanner::start(store.clone(), library(root)).unwrap();
    let running = scanner.request(Trigger::Manual, folder(root, "A")).unwrap();
    let queued = scanner.request(Trigger::Manual, folder(root, "B")).unwrap();
    let saved_running =
        || rows.scans().iter().any(|s| s.id == running && s.state == ScanState::Running);
    wait_until("the scan of A never started", saved_running);
    (scanner, thread, running, queued)
}

/// A scan of the folder `name` under `root`, and all below it.
fn folder(root: &Path, name: &str) -> Vec<Scope> {
    vec![Scope::folder(root, root.join(name), Depth::Subtree)]
}

/// A request a queued scan already covers joins it without writing a row.
#[test]
fn a_request_joining_a_queued_scan_writes_no_row() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let rows = Arc::new(BareStore::default());
    let store = Arc::new(Hooked::new(rows.clone()));
    let (scanner, thread, _, queued) = parked(&root, &store, &rows);
    let written = rows.scans().len();

    let within = vec![Scope::folder(&root, root.join("B/Y"), Depth::Directory)];
    assert_eq!(scanner.request(Trigger::Watch, within), Some(queued));
    assert_eq!(rows.scans().len(), written);
    store.release();
    scanner.wait_idle();
    drop(thread);
}

/// A request the store cannot record is not queued and is not retried: its work waits for the
/// next trigger.
#[test]
fn a_request_the_store_cannot_record_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let failing = AtomicBool::new(true);
    let rows = Arc::new(BareStore::default());
    let store = Arc::new(Hooked::new(rows.clone()).on_scan_write(move || {
        match failing.swap(false, Ordering::SeqCst) {
            true => Err(StoreError("disk full".into())),
            false => Ok(()),
        }
    }));
    let options = ScanOptions { retry_wait: Duration::ZERO, ..Default::default() };
    let (scanner, thread) = Scanner::start_with(store.clone(), library(&root), options).unwrap();

    assert_eq!(scanner.scan_library(Trigger::Manual), None);
    thread::sleep(Duration::from_millis(200));
    assert!(scanner.is_idle());
    assert!(rows.scans().is_empty());
    drop(thread);
}

/// Two requests for the same work at once queue it once.
#[test]
fn requests_at_once_for_the_same_work_queue_it_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let rows = Arc::new(BareStore::default());
    let store = Arc::new(Hooked::new(rows.clone()).on_scan_write(|| {
        thread::sleep(Duration::from_millis(100));
        Ok(())
    }));
    let (scanner, thread, _, _) = parked(&root, &store, &rows);
    let other = scanner.clone();
    let scopes = folder(&root, "C");
    let first = thread::spawn(move || other.request(Trigger::Watch, scopes));
    let second = scanner.request(Trigger::Watch, folder(&root, "C"));
    assert_eq!(first.join().unwrap(), second);
    store.release();
    scanner.wait_idle();
    drop(thread);
}
