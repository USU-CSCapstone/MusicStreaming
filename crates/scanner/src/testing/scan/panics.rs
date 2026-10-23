//! A panic while scanning: one in a parser makes its file a problem and the scan goes on; one
//! anywhere else ends the scan at its last flushed cursor, as a failed batch does, and the worker
//! goes on to the next (`requirements/scanning.md` §10). The failed batch is tested here too.

use std::sync::Once;

use lofty::resolve::register_custom_resolver;

use super::*;
use crate::clock::now_ms;
use crate::problems::ProblemKind;
use crate::queue::Scanner;
use crate::testing::wait_until;
use resolver::{PANICS, Panics};

mod resolver;

/// A panic in lofty's parser makes its file a malformed-tags problem with a short fixed reason,
/// not the panic's message, and the scan indexes the files around it and completes.
pub fn a_parser_panic_is_a_problem_and_the_scan_completes<S: InspectStore>(make: Factory<S>) {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| register_custom_resolver::<Panics>("Panics"));
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "C", "Z", 1);
    let bad = root.join("B/Y/01 - Bad.flac");
    fs::create_dir_all(bad.parent().unwrap()).unwrap();
    fs::write(&bad, PANICS).unwrap();
    let lib = library(&root);
    let store = make(&lib);

    let scan = scan_flushing_often(&store, &lib);
    assert_eq!(scan.state, ScanState::Completed, "{scan:?}");
    let [problem] = store.inspect_problems(lib.id).try_into().unwrap();
    assert_eq!(
        (&problem.path, problem.kind, problem.detail.as_str()),
        (&bad, ProblemKind::MalformedTags, "the tag reader failed unexpectedly")
    );
    let tracks = [root.join("A/X/01 - Track 1.wav"), root.join("C/Z/01 - Track 1.wav")];
    assert_eq!(paths(&store, &lib), tracks);
}

/// A panic outside parsing, here in the store as B/Y's index is read, suspends the scan at
/// its last flushed cursor with the progress the store holds, after A/X, and the worker goes on
/// to run the next scan, which completes. Nothing waits to be retried.
pub fn a_panic_suspends_the_scan_and_the_worker_runs_the_next<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let panic_at = root.join("B/Y");
    let armed = AtomicBool::new(true);
    let on_folder = move |path: &Path| {
        if path == panic_at && armed.swap(false, Ordering::SeqCst) {
            panic!("a store bug");
        }
        Ok(())
    };
    let hooked = Arc::new(Hooked::new(store.clone()).on_folder(on_folder));
    let (scanner, thread) = Scanner::start_with(hooked, lib.clone(), batches_of(1)).unwrap();

    let idle = "the scanner never went idle";
    let panicked = scanner.scan_library(Trigger::Manual).unwrap();
    wait_until(idle, || scanner.is_idle());
    assert!(!scanner.retry_in(Duration::ZERO), "a panic left work to retry");
    let next = scanner.scan_library(Trigger::Manual).unwrap();
    wait_until(idle, || scanner.is_idle());
    drop(thread);

    assert_eq!(store.inspect_scans(lib.id).len(), 2);
    let (suspended, next_row) =
        (row(store.as_ref(), &lib, panicked), row(store.as_ref(), &lib, next));
    assert_eq!(
        (suspended.state, &suspended.cursor, &suspended.progress),
        (ScanState::Suspended, &after(&root, "A/X"), &one_added()),
        "{suspended:?}"
    );
    assert!(suspended.finished_at.is_some());
    assert_eq!(next_row.state, ScanState::Completed);
    assert_eq!(store.inspect_tracks(lib.id).len(), 2);
}

/// A scan cancelled just before a panic ends cancelled rather than suspended, so it never resumes,
/// at its last flushed cursor after A/X.
pub fn a_panic_after_a_cancel_ends_cancelled<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let stops = Stops::default();
    let panic_at = root.join("B/Y");
    let on_folder = |path: &Path| {
        if path == panic_at {
            stops.cancel.store(true, Ordering::SeqCst);
            panic!("a store bug after the admin cancelled");
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_folder(on_folder);
    let mut scan = new_scan(&hooked, &lib);
    run(&hooked, &lib, &mut scan, &batches_of(1), &stops);

    let row = only_row(store.as_ref(), &lib);
    assert_eq!((row.state, &row.cursor), (ScanState::Cancelled, &after(&root, "A/X")), "{row:?}");
}

/// A batch the store fails to persist, B/Y's, suspends the scan at the last one it did, A/X's,
/// with the progress the store holds and no current path.
pub fn a_failed_batch_suspends_at_the_last_persisted_cursor<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let applied = AtomicUsize::new(0);
    let hooked =
        Hooked::new(store.clone()).on_apply(|_| match applied.fetch_add(1, Ordering::SeqCst) {
            1 => Err(StoreError("disk full".into())),
            _ => Ok(()),
        });
    scan_flushing_often(&hooked, &lib);

    let row = only_row(store.as_ref(), &lib);
    assert_eq!(
        (row.state, &row.cursor, &row.progress),
        (ScanState::Suspended, &after(&root, "A/X"), &one_added()),
        "{row:?}"
    );
}

/// A resumed scan that panics before its first flush suspends with exactly the cursor and
/// progress it resumed with, which the store still holds.
pub fn a_resumed_scan_that_panics_keeps_its_starting_cursor<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let panic_at = root.join("B/Y");
    let on_folder = |path: &Path| {
        if path == panic_at {
            panic!("a store bug");
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_folder(on_folder);
    let mut resumed = Scan {
        state: ScanState::Running,
        started_at: Some(now_ms()),
        progress: one_added(),
        cursor: after(&root, "A/X"),
        ..new_scan(&hooked, &lib)
    };
    hooked.update_scan(&resumed).unwrap();
    run(&hooked, &lib, &mut resumed, &batches_of(1000), &Stops::default());

    let row = only_row(store.as_ref(), &lib);
    assert_eq!(
        (row.state, &row.cursor, &row.progress),
        (ScanState::Suspended, &after(&root, "A/X"), &one_added()),
        "{row:?}"
    );
}

/// The progress the store holds once A/X's one track is added.
fn one_added() -> ScanProgress {
    ScanProgress { files_seen: 1, files_processed: 1, added: 1, ..Default::default() }
}
