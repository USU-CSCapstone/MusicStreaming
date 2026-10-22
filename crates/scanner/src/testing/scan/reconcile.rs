//! Each folder reconciled against its listing as the walk lists it: unchanged files write nothing,
//! a folder gone from its parent's listing takes its tracks, and a folder that cannot be listed
//! keeps them, even across a resume (`design/scanning.md` §8).

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;

use super::unlistable::Locked;
use super::*;
use crate::scope::Depth;
use crate::testing::{live, scan_library, scan_once, tag_wav};

/// `store`, keeping a copy of every batch applied through it in `batches` for a test to read.
fn recording<'a>(store: Arc<dyn Store>, batches: &'a Mutex<Vec<Batch>>) -> Hooked<'a> {
    Hooked::new(store).on_apply(move |batch: &Batch| {
        batches.lock().unwrap().push(batch.clone());
        Ok(())
    })
}

/// A rescan of an unchanged library writes nothing for its files: every batch it applies holds
/// only folders whose problems to clear, and the feed stays where it was. A file changed in place
/// is then the one thing the next scan writes.
pub fn an_unchanged_rescan_writes_nothing<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    make_album(&root, "A", "Y", 1);
    make_album(&root, "B", "Z", 2);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    scan_flushing_often(store.as_ref(), &lib);
    let head = store.feed_head(lib.id);
    let batches = Mutex::new(Vec::new());
    let hooked = recording(store.clone(), &batches);

    let scan = scan_flushing_often(&hooked, &lib);
    assert_eq!((scan.state, scan.progress.files_seen), (ScanState::Completed, 5));
    for batch in batches.lock().unwrap().drain(..) {
        assert_eq!((batch.len(), batch.changed_unread.len()), (0, 0), "{batch:?}");
    }
    assert_eq!(store.feed_head(lib.id), head);

    let changed = track(&root, "B/Z", 2);
    tag_wav(&changed, "Retagged", "B", "Z");
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    fs::File::open(&changed).unwrap().set_modified(later).unwrap();
    let scan = scan_flushing_often(&hooked, &lib);
    assert_eq!(scan.progress.updated, 1, "{:?}", scan.progress);
    let written: Vec<PathBuf> = batches
        .lock()
        .unwrap()
        .iter()
        .flat_map(|b| {
            assert_eq!(b.len(), b.upserts.len(), "{b:?}");
            b.upserts.iter().map(|t| t.path.clone())
        })
        .collect();
    assert_eq!(written, [changed]);
}

/// A folder gone from its parent's listing takes every track under it, at any depth, whether a
/// library scan or a Directory scope on the parent lists it, while the folders beside it keep
/// theirs.
pub fn a_folder_gone_from_its_parents_listing_takes_its_tracks<S: InspectStore>(make: Factory<S>) {
    for parent_only in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        make_album(&root, "A", "X", 1);
        make_album(&root, "A/Y", "Z", 2);
        write_small_wav(&root.join("A/Y/loose.wav"));
        make_album(&root, "A", "Y 2", 1);
        let lib = library(&root);
        let store = make(&lib);
        scan_library(&store, &lib);
        fs::remove_dir_all(root.join("A/Y")).unwrap();

        let scan = if parent_only {
            let scope = Scope::folder(&root, root.join("A"), Depth::Directory);
            scan_once(&store, &lib, Trigger::Watch, vec![scope])
        } else {
            scan_library(&store, &lib)
        };
        assert_eq!((scan.state, scan.progress.missing), (ScanState::Completed, 3), "{scan:?}");
        let live: Vec<PathBuf> = live(&store, &lib).into_iter().map(|t| t.path).collect();
        assert_eq!(live, [track(&root, "A/X", 1), track(&root, "A/Y 2", 1)], "{parent_only}");
    }
}

/// A scan stopped partway through a known folder reconciles none of it: the files it had not
/// reached are not marked missing, and the resume, which walks the folder again, finds nothing
/// gone. The stop comes as the batch holding a new file, read first, is stored.
pub fn a_scan_stopped_partway_through_a_folder_marks_nothing_there<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 3);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    scan_flushing_often(store.as_ref(), &lib);
    let added = root.join("A/X/00 - New.wav");
    write_small_wav(&added);
    let stops = Stops::default();
    let hooked = Hooked::new(store.clone()).on_apply(|batch: &Batch| {
        if batch.upserts.iter().any(|t| t.path == added) {
            stops.shutdown.store(true, Ordering::SeqCst);
        }
        Ok(())
    });

    let mut scan = new_scan(&hooked, &lib);
    run(&hooked, &lib, &mut scan, &batches_of(1), &stops);
    let mut stopped = row(store.as_ref(), &lib, scan.id);
    assert_eq!((stopped.state, stopped.progress.missing), (ScanState::Running, 0));
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));
    run(store.as_ref(), &lib, &mut stopped, &batches_of(1), &Stops::default());

    let resumed = row(store.as_ref(), &lib, scan.id);
    let ended = (resumed.state, resumed.progress.missing, resumed.progress.added);
    assert_eq!(ended, (ScanState::Completed, 0, 1));
    assert_eq!(live(store.as_ref(), &lib).len(), 4);
}

/// A folder that cannot be listed keeps its tracks though the scan that met it resumes after a
/// shutdown, forgetting what it could not read: B/Y's parent still lists it. Skipped, saying so,
/// where permissions do not stop this user reading, as for root.
pub fn an_unlistable_folder_keeps_its_tracks_across_a_resume<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 2);
    make_album(&root, "C", "Z", 1);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    scan_flushing_often(store.as_ref(), &lib);
    let _locked = Locked(vec![root.join("B/Y")]);
    fs::set_permissions(root.join("B/Y"), Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(root.join("B/Y")).is_ok() {
        eprintln!("skipped: permissions do not stop this user reading a folder (running as root?)");
        return;
    }
    let stops = Stops::default();
    let stop_at = root.join("C");
    let hooked = Hooked::new(store.clone()).on_folder(|path| {
        if path == stop_at {
            stops.shutdown.store(true, Ordering::SeqCst);
        }
        Ok(())
    });

    let mut scan = new_scan(&hooked, &lib);
    run(&hooked, &lib, &mut scan, &batches_of(1), &stops);
    let mut stopped = row(store.as_ref(), &lib, scan.id);
    assert_eq!((stopped.state, &stopped.cursor), (ScanState::Running, &after(&root, "C")));
    run(store.as_ref(), &lib, &mut stopped, &batches_of(1), &Stops::default());

    let resumed = row(store.as_ref(), &lib, scan.id);
    assert_eq!((resumed.state, resumed.progress.missing), (ScanState::Completed, 0));
    assert_eq!(live(store.as_ref(), &lib).len(), 4);
}
