//! What a scan leaves undone for a retry: the work a suspended scan did not finish, and nothing
//! for a scan that stops for any other reason (`design/scanning.md` §9).

use super::*;

/// Two roots, `a` and `b`, with one album each, as one library.
fn two_roots(tmp: &Path) -> (PathBuf, PathBuf, LibraryConfig) {
    let (a, b) = (tmp.join("a"), tmp.join("b"));
    make_album(&a, "A", "X", 1);
    make_album(&b, "B", "Y", 1);
    let lib = LibraryConfig { roots: vec![a.clone(), b.clone()], ..library(&a) };
    (a, b, lib)
}

/// A batch that fails in the second root leaves that root undone, from the cursor the store
/// holds, and not the first, which completed.
pub fn a_failed_batch_leaves_the_scopes_from_its_cursor_undone<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b, lib) = two_roots(tmp.path());
    let store = Arc::new(make(&lib));
    let fail_after = b.join("B/Y");
    let armed = AtomicBool::new(false);
    let on_folder = |path: &Path| {
        if path == fail_after {
            armed.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    let on_apply = |_: &_| {
        if armed.load(Ordering::SeqCst) {
            return Err(StoreError("disk full".into()));
        }
        Ok(())
    };
    let hooked = Hooked::new(store).on_apply(on_apply).on_folder(on_folder);
    let scopes = [Scope::root(&a), Scope::root(&b)];
    let mut scan = hooked.create_scan(lib.id, Trigger::Manual, &scopes).unwrap();

    let undone = run(&hooked, &lib, &mut scan, &batches_of(1), &Stops::default());
    assert_eq!(scan.state, ScanState::Suspended);
    assert_eq!(scan.cursor.as_ref().map(|c| c.scope_index), Some(1), "{scan:?}");
    assert_eq!(undone, Undone { unanswered: Vec::new(), failed: vec![Scope::root(&b)] });
}

/// A scan whose first root does not answer leaves nothing undone when a shutdown stops it in a
/// later root: its row stays running, for the next start to resume.
pub fn a_stopped_scan_leaves_nothing_undone<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b, mut lib) = two_roots(tmp.path());
    let gone = tmp.path().join("gone");
    lib.roots.push(gone.clone());
    let store = Arc::new(make(&lib));
    let stops = Stops::default();
    let stop_at = b.join("B/Y");
    let on_folder = |path: &Path| {
        if path == stop_at {
            stops.shutdown.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_folder(on_folder);
    let scopes = [Scope::root(&gone), Scope::root(&a), Scope::root(&b)];
    let mut scan = hooked.create_scan(lib.id, Trigger::Manual, &scopes).unwrap();

    let undone = run(&hooked, &lib, &mut scan, &batches_of(1), &stops);
    let state = only_row(store.as_ref(), &lib).state;
    assert_eq!((state, undone), (ScanState::Running, Undone::default()));
}
