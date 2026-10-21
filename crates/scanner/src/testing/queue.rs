//! Scenarios for the scan queue: scan rows, resuming from a cursor, and how requests join or
//! queue behind the scans already there (`design/scanning.md` §9).

use std::fs;
use std::sync::Arc;

use super::{
    Factory, InspectStore, Parked, library, live, make_album, wait_until, wait_until_running,
    write_small_wav,
};
use crate::clock::now_ms;
use crate::library::LibraryConfig;
use crate::queue::Scanner;
use crate::scan::{Cursor, Scan, ScanProgress, ScanState, Trigger};
use crate::scope::{Depth, Scope};

/// A scan's start and finish times read back as they were saved.
pub fn scan_times_read_back<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    let lib = library(&root);
    let mut scan = Scan {
        id: 0,
        library: lib.id,
        trigger: Trigger::Manual,
        scopes: vec![Scope::root(&root)],
        state: ScanState::Running,
        started_at: None,
        finished_at: None,
        progress: ScanProgress::default(),
        cursor: None,
    };
    scan.id = store.create_scan(&scan).unwrap();
    scan.started_at = Some(1_700_000_000_123);
    scan.finished_at = Some(1_700_000_000_456);
    store.update_scan(&scan).unwrap();
    let [read] = store.interrupted_scans(lib.id).unwrap().try_into().unwrap();
    assert_eq!((read.started_at, read.finished_at), (scan.started_at, scan.finished_at));
}

/// A scan interrupted mid-way resumes from its cursor when the scanner starts. Work that a
/// queued scan already covers joins it instead of queuing again, while work the running scan
/// covers queues behind it, since the walk may already have passed it.
pub fn queue_runs_resumes_and_coalesces<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    make_album(&root, "B", "Y", 2);
    make_album(&root, "C", "Z", 2);
    let store: Arc<S> = Arc::new(make(&root));
    let lib = library(&root);

    // A scan interrupted after finishing directory "A/X" is left Running
    // with a cursor. Starting the scanner resumes it.
    let interrupted = Scan {
        id: 0,
        library: lib.id,
        trigger: Trigger::Initial,
        scopes: vec![Scope::root(&root)],
        state: ScanState::Running,
        started_at: Some(now_ms()),
        finished_at: None,
        progress: ScanProgress::default(),
        cursor: Some(Cursor { scope_index: 0, after_directory: Some(root.join("A/X")) }),
    };
    let mut interrupted_row = interrupted.clone();
    interrupted_row.id = store.create_scan(&interrupted).unwrap();
    store.update_scan(&interrupted_row).unwrap();

    let parked = Parked::new(store.clone());
    let (scanner, thread) = Scanner::start(parked.clone(), lib.clone()).unwrap();
    scanner.wait_idle();
    let scans = store.inspect_scans(lib.id);
    let resumed = scans.iter().find(|s| s.id == interrupted_row.id).unwrap();
    assert_eq!(resumed.state, ScanState::Completed);
    assert_eq!(resumed.progress.files_seen, 4, "{:?}", resumed.progress);

    // A scan of "C", held by the store, holds what is requested after it in the queue.
    parked.hold();
    let held = scanner
        .request(Trigger::Manual, vec![Scope::folder(&root, root.join("C"), Depth::Subtree)])
        .unwrap();
    wait_until_running(&scanner, held);
    let z = || vec![Scope::folder(&root, root.join("C/Z"), Depth::Directory)];
    let behind = scanner.request(Trigger::Watch, z()).unwrap();
    assert_ne!(behind, held, "the running scan takes no new work, even work it covers");
    assert_eq!(
        scanner.request(Trigger::Watch, z()).unwrap(),
        behind,
        "it joins the one queued behind"
    );
    let dropped = scanner
        .request(Trigger::Manual, vec![Scope::folder(&root, root.join("B"), Depth::Subtree)])
        .unwrap();
    assert!(scanner.cancel(dropped));
    assert!(scanner.queued().iter().all(|s| s.id != dropped), "{:?}", scanner.queued());
    assert!(!scanner.cancel(dropped), "a cancelled scan is gone");
    let stored = store.inspect_scans(lib.id).into_iter().find(|s| s.id == dropped).unwrap();
    assert_eq!(stored.state, ScanState::Cancelled);
    assert!(stored.finished_at.is_some());
    let a = scanner
        .request(Trigger::Manual, vec![Scope::folder(&root, root.join("A"), Depth::Subtree)])
        .unwrap();
    let b = scanner
        .request(Trigger::Manual, vec![Scope::folder(&root, root.join("A/X"), Depth::Directory)])
        .unwrap();
    let all = scanner.scan_library(Trigger::Scheduled).unwrap();
    let again = scanner.scan_library(Trigger::Manual).unwrap();
    assert_eq!(a, b, "a covered scope joins the queued scan");
    assert_eq!(all, again);
    parked.release();
    scanner.wait_idle();
    let final_scans = store.inspect_scans(lib.id);
    let state = |id| final_scans.iter().find(|s| s.id == id).unwrap().state;
    assert_eq!(state(held), ScanState::Completed);
    assert_eq!(state(behind), ScanState::Cancelled, "the library scan replaced it");
    assert_eq!(state(a), ScanState::Cancelled, "the library scan replaced it");
    assert_eq!(state(all), ScanState::Completed);
    assert_eq!(live(store.as_ref(), &lib).len(), 6);
    drop(thread);
}

/// A resumed scan waiting in the queue takes no new work, since its walk has passed what lies
/// before its cursor: the startup scan queues anew, so a file added there while the server was
/// down is indexed. Two library scans interrupted after "B/Y" are resumed; whichever runs first
/// is held by the store, and the other waits in the queue.
pub fn a_resumed_scan_takes_no_new_work<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    make_album(&root, "C", "Z", 1);
    let store: Arc<S> = Arc::new(make(&root));
    let lib = library(&root);
    for _ in 0..2 {
        let mut interrupted = Scan {
            id: 0,
            library: lib.id,
            trigger: Trigger::Initial,
            scopes: vec![Scope::root(&root)],
            state: ScanState::Running,
            started_at: Some(now_ms()),
            finished_at: None,
            progress: ScanProgress::default(),
            cursor: Some(Cursor { scope_index: 0, after_directory: Some(root.join("B/Y")) }),
        };
        interrupted.id = store.create_scan(&interrupted).unwrap();
        store.update_scan(&interrupted).unwrap();
    }

    let parked = Parked::new(store.clone());
    parked.hold();
    let (scanner, thread) = Scanner::start(parked.clone(), lib.clone()).unwrap();
    wait_until(&scanner, "no resumed scan started", |scanner| scanner.running().is_some());
    let [waiting] = scanner.queued().try_into().unwrap();
    let added = root.join("A/X/02 - Added.wav");
    write_small_wav(&added);
    let startup = scanner.scan_library(Trigger::Initial).unwrap();
    assert_ne!(startup, waiting.id, "the startup scan does not join a resumed scan");
    parked.release();
    scanner.wait_idle();
    drop(thread);

    assert!(live(store.as_ref(), &lib).iter().any(|t| t.path == added));
    let scans = store.inspect_scans(lib.id);
    let state = |id| scans.iter().find(|s| s.id == id).unwrap().state;
    assert_eq!(state(waiting.id), ScanState::Cancelled, "the startup scan replaced it");
    assert_eq!(state(startup), ScanState::Completed);
}

/// A reconfiguration while a library scan runs queues a scan of its own, which applies the
/// new excludes: tracks under an added exclude end up missing. The library is indexed first,
/// so the outcome holds whichever configuration the parked scan read when it started.
pub fn a_reconfigure_during_a_scan_applies_its_excludes<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "backup", "X", 1);
    let store: Arc<S> = Arc::new(make(&root));
    let lib = library(&root);
    let parked = Parked::new(store.clone());
    let (scanner, thread) = Scanner::start(parked.clone(), lib.clone()).unwrap();
    scanner.scan_library(Trigger::Initial).unwrap();
    scanner.wait_idle();
    parked.hold();
    let running = scanner.scan_library(Trigger::Scheduled).unwrap();
    wait_until_running(&scanner, running);

    let excluding = LibraryConfig { excludes: vec!["backup".into(), "backup/**".into()], ..lib };
    let reconfigured = scanner.reconfigure(excluding.clone()).unwrap();
    assert_ne!(reconfigured, running, "a scan that has started takes no new work");
    parked.release();
    scanner.wait_idle();
    drop(thread);

    let tracks = store.inspect_tracks(excluding.id);
    let missing = |path: &str| tracks.iter().find(|t| t.path == root.join(path)).unwrap().missing;
    assert!(!missing("A/X/01 - Track 1.wav"));
    assert!(missing("backup/X/01 - Track 1.wav"));
}
