//! Scenarios for stopping the scanner: a shutdown leaves its scans to resume, and a cancel ends
//! a scan for good (`design/scanning.md` §9).

use std::sync::Arc;
use std::time::SystemTime;

use super::{Factory, InspectStore, Parked, library, live, make_album, wait_until_running};
use crate::queue::Scanner;
use crate::scan::ScanOptions;
use crate::types::*;

/// A shutdown stops the running scan without ending it: the row stays running at its cursor and
/// a queued scan stays queued, so the next start resumes both. The running scan is one a previous
/// process left after "A/X", held by the store before it reads anything more, and
/// resuming it never reads what lies before its cursor.
pub fn a_shutdown_leaves_its_scans_to_resume<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    make_album(&root, "C", "Z", 1);
    let store: Arc<S> = Arc::new(make(&root));
    let lib = library(&root);
    let cursor = Some(Cursor { scope_index: 0, after_directory: Some(root.join("A/X")) });
    let mut interrupted = Scan {
        id: 0,
        library: lib.id,
        trigger: Trigger::Initial,
        scopes: vec![Scope::root(&root)],
        state: ScanState::Running,
        started_at: Some(SystemTime::now()),
        finished_at: None,
        progress: ScanProgress::default(),
        cursor: cursor.clone(),
    };
    interrupted.id = store.create_scan(&interrupted).unwrap();
    store.update_scan(&interrupted).unwrap();

    let parked = Parked::new(store.clone());
    parked.hold();
    let scanner = Scanner::start(parked.clone(), lib.clone(), ScanOptions::default());
    wait_until_running(&scanner, interrupted.id);
    let queued = scanner
        .request(Trigger::Manual, vec![Scope::folder(&root, root.join("C"), Depth::Subtree)])
        .unwrap();
    scanner.stop();
    parked.release();
    scanner.shutdown();

    let scans = store.inspect_scans(lib.id);
    let row = |id| scans.iter().find(|s| s.id == id).unwrap();
    let stopped = row(interrupted.id);
    assert_eq!((stopped.state, &stopped.cursor), (ScanState::Running, &cursor), "{stopped:?}");
    assert_eq!(stopped.finished_at, None);
    assert_eq!(row(queued).state, ScanState::Queued);

    let restarted = Scanner::start(store.clone(), lib.clone(), ScanOptions::default());
    restarted.wait_idle();
    restarted.shutdown();
    let scans = store.inspect_scans(lib.id);
    let row = |id| scans.iter().find(|s| s.id == id).unwrap();
    let resumed = row(interrupted.id);
    assert_eq!(resumed.state, ScanState::Completed);
    assert_eq!(resumed.progress.files_seen, 2, "{:?}", resumed.progress);
    assert_eq!(row(queued).state, ScanState::Completed);
    let paths: Vec<_> = live(store.as_ref(), &lib).into_iter().map(|t| t.path).collect();
    assert_eq!(paths, [root.join("B/Y/01 - Track 1.wav"), root.join("C/Z/01 - Track 1.wav")]);
}

/// A cancelled scan ends cancelled, even when a shutdown follows before it stops, and the next
/// start resumes neither. Each scan is held by the store before it reads anything.
pub fn a_cancelled_scan_stays_cancelled<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    let store: Arc<S> = Arc::new(make(&root));
    let lib = library(&root);
    let parked = Parked::new(store.clone());
    let scanner = Scanner::start(parked.clone(), lib.clone(), ScanOptions::default());

    parked.hold();
    let cancelled = scanner.scan_library(Trigger::Manual).unwrap();
    wait_until_running(&scanner, cancelled);
    assert!(scanner.cancel(cancelled));
    parked.release();
    scanner.wait_idle();

    parked.hold();
    let then_shut_down = scanner.scan_library(Trigger::Manual).unwrap();
    wait_until_running(&scanner, then_shut_down);
    assert!(scanner.cancel(then_shut_down));
    scanner.stop();
    parked.release();
    scanner.shutdown();

    let restarted = Scanner::start(store.clone(), lib.clone(), ScanOptions::default());
    restarted.wait_idle();
    restarted.shutdown();
    let scans = store.inspect_scans(lib.id);
    for id in [cancelled, then_shut_down] {
        let scan = scans.iter().find(|s| s.id == id).unwrap();
        assert_eq!(scan.state, ScanState::Cancelled, "{scan:?}");
        assert!(scan.finished_at.is_some());
    }
    assert!(live(store.as_ref(), &lib).is_empty());
}
