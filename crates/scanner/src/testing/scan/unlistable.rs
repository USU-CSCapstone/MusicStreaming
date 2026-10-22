//! A folder the walk cannot list counts as one problem, once the cursor passes it or the scan
//! ends, however often the scan resumes (`design/scanning.md` §9, §13).

use std::fs::Permissions;
use std::os::unix::fs::{PermissionsExt, symlink};

use super::*;
use crate::scope::Depth;
use crate::testing::BareStore;

/// Folders made unlistable, listable again when dropped, so the temporary directory can be
/// removed even after a failed assertion.
pub(super) struct Locked(pub(super) Vec<PathBuf>);

impl Drop for Locked {
    /// Restores each folder's mode, reporting any it cannot.
    fn drop(&mut self) {
        for folder in &self.0 {
            if let Err(error) = fs::set_permissions(folder, Permissions::from_mode(0o755)) {
                eprintln!("could not restore {}: {error}", folder.display());
            }
        }
    }
}

/// A/X with one track, then B, which cannot be listed, C with three tracks directly in it and
/// two in C/Z, and D, which cannot be listed either. Returns B and D locked, or nothing where
/// permissions do not stop this user reading, as for root.
fn unlistable_folders(root: &Path) -> Option<Locked> {
    make_album(root, "A", "X", 1);
    (1..=3).for_each(|n| write_small_wav(&track(root, "C", n)));
    make_album(root, "C", "Z", 2);
    let locked = Locked(vec![root.join("B"), root.join("D")]);
    for folder in &locked.0 {
        fs::create_dir(folder).unwrap();
        fs::set_permissions(folder, Permissions::from_mode(0o000)).unwrap();
    }
    if fs::read_dir(&locked.0[0]).is_ok() {
        eprintln!("skipped: permissions do not stop this user reading a folder (running as root?)");
        return None;
    }
    Some(locked)
}

/// B, which could not be listed, counts once the cursor passes it, and not before. A shutdown as
/// the scan starts on C, the first folder after B, leaves B uncounted, since the resume meets it
/// again; one as it starts on C/Z, once C is complete, leaves it counted. The scan, resumed
/// again, counts D, met after the last folder, as the walk ends, and ends with the counts of one
/// never stopped.
pub fn an_unlistable_folder_is_counted_once_across_resumes<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let Some(_locked) = unlistable_folders(&root) else { return };
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let stops = Stops::default();
    let stop_at = [root.join("C"), root.join("C/Z")];
    let fired = [AtomicBool::new(false), AtomicBool::new(false)];
    let on_folder = |path: &Path| {
        for (at, fired) in stop_at.iter().zip(&fired) {
            if path == at && !fired.swap(true, Ordering::SeqCst) {
                stops.shutdown.store(true, Ordering::SeqCst);
            }
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_folder(on_folder);
    let mut row = new_scan(&hooked, &lib);
    let mut stopped = Vec::new();
    for _ in &stop_at {
        run(&hooked, &lib, &mut row, &batches_of(1000), &stops);
        stops.shutdown.store(false, Ordering::SeqCst);
        row = only_row(store.as_ref(), &lib);
        stopped.push((row.state, row.cursor.clone(), row.progress.problems));
    }
    run(&hooked, &lib, &mut row, &batches_of(1000), &stops);
    let uninterrupted = scan_flushing_often(&BareStore::default(), &lib);

    let running = ScanState::Running;
    let at_c = (running, after(&root, "C"), 1);
    assert_eq!(stopped, [(running, after(&root, "A/X"), 0), at_c]);
    let resumed = only_row(store.as_ref(), &lib);
    assert_eq!(resumed.state, ScanState::Completed);
    assert_eq!(resumed.progress, uninterrupted.progress);
    let counts = (resumed.progress.files_seen, resumed.progress.problems);
    assert_eq!(counts, (6, 2), "{:?}", resumed.progress);
}

/// A cancel in C, as the batch holding its second track is stored, counts B, which could not be
/// listed, though the cursor has not passed it, since a cancelled scan never resumes.
pub fn a_cancel_counts_an_unlistable_folder_it_passed<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let Some(_locked) = unlistable_folders(&root) else { return };
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let stops = Stops::default();
    let cancel_at = track(&root, "C", 2);
    let on_apply = |batch: &Batch| {
        if batch.upserts.iter().any(|t| t.path == cancel_at) {
            stops.cancel.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_apply(on_apply);
    let mut scan = new_scan(&hooked, &lib);
    run(&hooked, &lib, &mut scan, &batches_of(1), &stops);

    let row = only_row(store.as_ref(), &lib);
    assert_eq!(row.state, ScanState::Cancelled);
    assert_eq!((row.progress.files_seen, row.progress.problems), (3, 1), "{:?}", row.progress);
}

/// An unlistable folder met in a scope whose root went before reconcile counts once a later scope
/// moves the cursor past it, and one met as a scope's walk ends counts then, each once. The first
/// root completes, counting B, D, and its symlink L; the second goes as its walk is flushed,
/// counting B and leaving D until the last scope, L itself, refused, moves the cursor. The second
/// root is left for a retry to walk again, since no resume from the cursor would.
pub fn unlistable_folders_count_once_the_cursor_passes_their_scope<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let [kept, gone] = ["kept", "gone"].map(|name| tmp.path().join(name));
    let (Some(_kept), Some(_gone)) = (unlistable_folders(&kept), unlistable_folders(&gone)) else {
        return;
    };
    symlink(kept.join("C"), kept.join("A/L")).unwrap();
    let lib = LibraryConfig { roots: vec![kept.clone(), gone.clone()], ..library(&kept) };
    let store = Arc::new(make(&lib));
    let (away, last) = (tmp.path().join("away"), gone.join("C/Z"));
    // The second root goes at the first flush after the scan starts on its last folder, as its
    // walk ends.
    let walked = AtomicBool::new(false);
    let on_folder = |path: &Path| {
        if path == last {
            walked.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    let on_apply = |_: &_| {
        if walked.load(Ordering::SeqCst) && gone.exists() {
            fs::rename(&gone, &away).unwrap();
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_apply(on_apply).on_folder(on_folder);
    let link = Scope::folder(&kept, kept.join("A/L"), Depth::Subtree);
    let scopes = [Scope::root(&kept), Scope::root(&gone), link];
    let mut scan = hooked.create_scan(lib.id, Trigger::Manual, &scopes).unwrap();
    let undone = run(&hooked, &lib, &mut scan, &batches_of(1000), &Stops::default());
    // Put back, so the locked folders are found to restore.
    fs::rename(&away, &gone).unwrap();

    let row = only_row(store.as_ref(), &lib);
    assert_eq!(row.state, ScanState::Suspended);
    assert_eq!(row.progress.problems, 6, "{:?}", row.progress);
    assert_eq!(undone.unanswered, [Scope::root(&gone)]);
    assert_eq!(undone.failed, []);
}

/// A scope whose root goes as its walk is flushed ends the scan suspended with the cursor still
/// in it, counting D, met after its last folder, as the scan ends, and leaves the scope for a
/// retry, which counts what it meets for itself.
pub fn a_root_gone_after_its_walk_counts_its_last_unlistable_folder_as_the_scan_ends<
    S: InspectStore + 'static,
>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let Some(_locked) = unlistable_folders(&root) else { return };
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let (away, last) = (tmp.path().join("away"), root.join("C/Z"));
    // The root goes at the first flush after the scan starts on its last folder, as its walk
    // ends.
    let walked = AtomicBool::new(false);
    let on_folder = |path: &Path| {
        if path == last {
            walked.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    let on_apply = |_: &_| {
        if walked.load(Ordering::SeqCst) && root.exists() {
            fs::rename(&root, &away).unwrap();
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_apply(on_apply).on_folder(on_folder);
    let mut scan = new_scan(&hooked, &lib);
    let undone = run(&hooked, &lib, &mut scan, &batches_of(1000), &Stops::default());
    // Put back, so the locked folders are found to restore.
    fs::rename(&away, &root).unwrap();

    let row = only_row(store.as_ref(), &lib);
    let ended = (row.state, &row.cursor, row.progress.problems);
    assert_eq!(ended, (ScanState::Suspended, &after(&root, "C/Z"), 2), "{row:?}");
    assert_eq!(undone.unanswered, [Scope::root(&root)]);
    assert_eq!(undone.failed, []);
}
