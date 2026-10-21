//! What the scanner does when the store fails: a scan suspends, a request queues nothing at
//! once, a background job backs off, and one unreadable scan row hides only itself and is marked
//! suspended. Each failure is a trigger that aborts the write it guards, as a full disk would.

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use jewelcase_ffmpeg::{Config, Ffmpeg};
use jewelcase_scanner::testing::{self, InspectStore};
use jewelcase_scanner::*;

use super::{count, make};
use crate::db::store::SqliteStore;

/// Makes every write `sql` names fail, by a trigger that aborts it.
fn fail(store: &SqliteStore, sql: &str) {
    let trigger =
        format!("CREATE TRIGGER failing {sql} BEGIN SELECT RAISE(ABORT, 'disk full'); END");
    store.db().write_blocking(move |tx| tx.execute_batch(&trigger)).unwrap();
}

/// Lets the writes [`fail`] stopped through again.
fn heal(store: &SqliteStore) {
    store.db().write_blocking(|tx| tx.execute_batch("DROP TRIGGER failing")).unwrap();
}

/// A scan whose reconcile cannot mark a deleted file's track missing ends suspended, with
/// nothing counted missing, rather than completed as though nothing were missing.
#[test]
fn a_reconcile_the_store_fails_suspends_the_scan() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 2);
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    fs::remove_file(root.join("A/X/02 - Track 2.wav")).unwrap();
    fail(&store, "BEFORE UPDATE OF missing_since ON tracks WHEN NEW.missing_since IS NOT NULL");

    let scan = testing::scan_library(&store, &lib);
    assert_eq!((scan.state, scan.progress.missing), (ScanState::Suspended, 0), "{scan:?}");
    let rows = store.inspect_scans(lib.id);
    assert_eq!(rows.iter().find(|row| row.id == scan.id).unwrap().state, ScanState::Suspended);
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));

    heal(&store);
    let scan = testing::scan_library(&store, &lib);
    assert_eq!((scan.state, scan.progress.missing), (ScanState::Completed, 1), "{scan:?}");
}

/// A scan whose row cannot be saved as it starts, as it saves its progress, or as it completes
/// ends suspended at its last saved cursor, rather than carrying on or completing unsaved.
#[test]
fn a_scan_whose_row_the_store_cannot_save_ends_suspended() {
    let saves = [
        "NEW.state = 'running' AND NEW.files_seen = 0",
        "NEW.state = 'running' AND NEW.files_seen > 0",
        "NEW.state = 'completed'",
    ];
    for save in saves {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        testing::make_album(&root, "A", "X", 1);
        let store = make(&root);
        let lib = testing::library(&root);
        fail(&store, &format!("BEFORE UPDATE ON scans WHEN {save}"));

        let scan = testing::scan_library(&store, &lib);
        assert_eq!(scan.state, ScanState::Suspended, "{save}");
        let [row] = store.inspect_scans(lib.id).try_into().unwrap();
        assert_eq!((row.id, row.state, row.progress), (scan.id, scan.state, scan.progress));
    }
}

/// A scan the store cannot record is never queued under a made-up id, and the queued scan it
/// would have replaced stays queued. The first scan is held by the store, so the second waits in
/// the queue.
#[test]
fn a_scan_the_store_cannot_record_is_not_queued() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    let store = Arc::new(make(&root));
    let lib = testing::library(&root);
    let parked = testing::Parked::new(store.clone());
    parked.hold();
    let scanner = Scanner::start(parked.clone(), lib.clone(), ScanOptions::default());
    scanner.scan_library(Trigger::Manual).unwrap();
    testing::wait_until(&scanner, "no scan started", |scanner| scanner.running().is_some());
    let folder = scanner.scan_folder(Trigger::Watch, &root.join("A")).unwrap();

    fail(&store, "BEFORE INSERT ON scans");
    assert_eq!(scanner.scan_library(Trigger::Manual), None);
    let queued: Vec<ScanId> = scanner.queued().iter().map(|scan| scan.id).collect();
    assert_eq!(queued, [folder]);
    heal(&store);
    parked.release();
    scanner.wait_idle();
    scanner.shutdown();
    assert_eq!(count(&store, "SELECT COUNT(*) FROM scans WHERE state = 'completed'"), 2);
}

/// One scan row that cannot be read is left out of those to resume, and marked suspended, since
/// it never can, and the others resume.
#[test]
fn an_unreadable_scan_row_does_not_stop_the_others_resuming() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
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
    let unreadable = store.create_scan(&scan).unwrap();
    scan.id = store.create_scan(&scan).unwrap();
    // A valid JSON array, as the schema checks, but not of scopes.
    let corrupt = "UPDATE scans SET scopes = '[1]' WHERE id = ?1";
    store.db().write_blocking(move |tx| tx.execute(corrupt, [unreadable])).unwrap();

    assert_eq!(store.interrupted_scans(lib.id).unwrap(), [scan.clone()]);
    let suspended = "SELECT COUNT(*) FROM scans WHERE state = 'suspended' AND finished_at > 0";
    assert_eq!(count(&store, suspended), 1);
    assert_eq!(store.interrupted_scans(lib.id).unwrap(), [scan]);
}

/// A file that cannot be opened, whose problem the store cannot record, is left as it was: the
/// pass backs off, and the next tries the file and records its problem, rather than finding it
/// waiting out a retry it never earned.
#[test]
fn a_problem_the_store_cannot_record_leaves_the_file_untried() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    let store = Arc::new(make(&root));
    let lib = testing::library(&root);
    testing::scan_library(store.as_ref(), &lib);
    fs::remove_file(root.join("A/X/01 - Track 1.wav")).unwrap();
    let analyzer = Analyzer::new(Ffmpeg::new(Config::default()), store.clone());

    fail(&store, "BEFORE INSERT ON scan_problems");
    assert_eq!(analyzer.run_once(lib.id, 10), RunReport { backed_off: true, ..Default::default() });
    heal(&store);
    assert_eq!(analyzer.run_once(lib.id, 10), RunReport { skipped: 1, ..Default::default() });
    assert_eq!(store.inspect_problems(lib.id).len(), 1);
}

/// A file that decodes, whose problem saying it could not be opened the store cannot clear, is
/// left unanalyzed: the pass backs off before keeping the result, so the next pass analyzes it
/// again and clears the problem, rather than leaving the problem beside a stored result.
#[test]
fn a_problem_the_store_cannot_clear_leaves_the_file_unanalyzed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    let store = Arc::new(make(&root));
    let lib = testing::library(&root);
    testing::scan_library(store.as_ref(), &lib);
    let (file, away) = (root.join("A/X/01 - Track 1.wav"), tmp.path().join("away.wav"));
    fs::rename(&file, &away).unwrap();
    let analyzer = Analyzer::new(Ffmpeg::new(Config::default()), store.clone())
        .with_retry_after(Duration::ZERO);
    assert_eq!(analyzer.run_once(lib.id, 10), RunReport { skipped: 1, ..Default::default() });
    assert_eq!(store.inspect_problems(lib.id).len(), 1);

    fs::rename(&away, &file).unwrap();
    fail(&store, "BEFORE DELETE ON scan_problems");
    assert!(analyzer.run_once(lib.id, 10).backed_off);
    heal(&store);
    assert_eq!(analyzer.run_once(lib.id, 10), RunReport { done: 1, ..Default::default() });
    assert!(store.inspect_problems(lib.id).is_empty());
}
