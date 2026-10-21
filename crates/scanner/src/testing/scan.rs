//! Scenarios that run one scan by hand, with the store hooked to stop the scan, fail it, or change
//! the world partway (`design/scanning.md` §5, §7, §9).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Factory, Hooked, InspectStore, library, make_album, write_small_wav};
use crate::library::LibraryConfig;
use crate::scan::{
    Cursor, Scan, ScanContext, ScanOptions, ScanProgress, ScanState, Trigger, Undone, run_scan,
};
use crate::scope::Scope;
use crate::store::{Store, StoreError};

mod checkpoints;
mod excludes;
mod failures;
mod panics;
mod resume;
mod undone;
mod unlistable;
pub use checkpoints::*;
pub use excludes::*;
pub use failures::*;
pub use panics::*;
pub use resume::*;
pub use undone::*;
pub use unlistable::*;

/// The flags that cancel a scan, or shut it down, from inside it.
#[derive(Default)]
struct Stops {
    cancel: AtomicBool,
    shutdown: AtomicBool,
}

/// A new manual scan of the library's one root, saved to `store`.
fn new_scan(store: &dyn Store, lib: &LibraryConfig) -> Scan {
    store.create_scan(lib.id, Trigger::Manual, &[Scope::root(&lib.roots[0])]).unwrap()
}

/// Batches of `batch_size` outcomes, never flushed on a timer.
fn batches_of(batch_size: usize) -> ScanOptions {
    ScanOptions { batch_size, batch_interval: Duration::MAX }
}

/// Runs `scan`, already saved to `store`, with `options`, cancelled or shut down once `stops`
/// says so, and returns what it left undone for a retry.
fn run(
    store: &dyn Store,
    lib: &LibraryConfig,
    scan: &mut Scan,
    options: &ScanOptions,
    stops: &Stops,
) -> Undone {
    let ctx = ScanContext {
        store,
        library: lib,
        cancel: &stops.cancel,
        shutdown: &stops.shutdown,
        options,
    };
    run_scan(&ctx, scan)
}

/// A cursor after the folder `relative` under `root`, in the first scope.
fn after(root: &Path, relative: &str) -> Option<Cursor> {
    Some(Cursor { scope_index: 0, after_directory: Some(root.join(relative)) })
}

/// The one scan row `store` holds.
fn only_row(store: &dyn InspectStore, lib: &LibraryConfig) -> Scan {
    let [row] = store.inspect_scans(lib.id).try_into().unwrap();
    row
}

/// The scan row `id` among those `store` holds.
fn row(store: &dyn InspectStore, lib: &LibraryConfig, id: i64) -> Scan {
    store.inspect_scans(lib.id).into_iter().find(|row| row.id == id).unwrap()
}

/// The paths of every track `store` holds, missing or not, sorted.
fn paths(store: &dyn InspectStore, lib: &LibraryConfig) -> Vec<PathBuf> {
    let mut paths: Vec<_> = store.inspect_tracks(lib.id).into_iter().map(|t| t.path).collect();
    paths.sort();
    paths
}

/// The `n`th track of the album `folder` that [`make_album`] wrote under `root`.
fn track(root: &Path, folder: &str, n: usize) -> PathBuf {
    root.join(folder).join(format!("{n:02} - Track {n}.wav"))
}

/// Runs a new scan of the whole library against `store`, flushing whenever the batch holds
/// a track or problem, and never on a timer.
fn scan_flushing_often(store: &dyn Store, lib: &LibraryConfig) -> Scan {
    let mut scan = new_scan(store, lib);
    run(store, lib, &mut scan, &batches_of(1), &Stops::default());
    scan
}

/// A stop flushes the batch in hand: a shutdown asked for while B/Y's track is read leaves the
/// row running at B/Y with A's and B's tracks stored, though no batch filled and no timer fired,
/// and C never read.
pub fn a_stop_flushes_the_batch_in_hand<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    make_album(&root, "C", "Z", 1);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let stops = Stops::default();
    let stop_at = root.join("B/Y/01 - Track 1.wav");
    let hooked = Hooked::new(store.clone()).on_lookup(|path| {
        if path == stop_at {
            stops.shutdown.store(true, Ordering::SeqCst);
        }
        Ok(())
    });
    let mut scan = new_scan(&hooked, &lib);
    run(&hooked, &lib, &mut scan, &batches_of(1000), &stops);

    let row = only_row(store.as_ref(), &lib);
    assert_eq!((row.state, &row.cursor), (ScanState::Running, &after(&root, "B/Y")), "{row:?}");
    assert_eq!(row.finished_at, None);
    assert_eq!(paths(store.as_ref(), &lib), [root.join("A/X/01 - Track 1.wav"), stop_at.clone()]);
}

/// A root that goes away after the walk began suspends the scan before reconcile, and nothing
/// the walk did not reach is marked missing (`requirements/scanning.md` §7).
pub fn a_root_gone_before_reconcile_suspends_and_marks_nothing<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("nas");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 2);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let away = tmp.path().join("away");
    let hooked = Hooked::new(store.clone()).on_apply(|_| {
        if root.exists() {
            fs::rename(&root, &away).unwrap();
        }
        Ok(())
    });
    scan_flushing_often(store.as_ref(), &lib);

    // The root goes once A/X's touch lands, so B and B/Y are never listed.
    let scan = scan_flushing_often(&hooked, &lib);
    assert!(!root.exists());
    assert_eq!(scan.progress.missing, 0);
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));
    assert_eq!(scan.state, ScanState::Suspended);
}

/// Sidecars are read only for a file past stage 2, so a folder of unchanged files opens none.
/// Here an unchanged track sits beside `artist.txt`, and a new track one folder below inherits
/// it. The biography changes as the unchanged track's touch is applied, so the new track, the
/// only one written, gets the later text only because the unchanged track never read the file.
pub fn an_unchanged_file_reads_no_sidecars<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let biography = root.join("A/artist.txt");
    write_small_wav(&root.join("A/01 - Kept.wav"));
    fs::write(&biography, "Read early.").unwrap();
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let written = Mutex::new(Vec::new());
    let hooked = Hooked::new(store.clone()).on_apply(|batch| {
        let upserts = batch.upserts.iter();
        written
            .lock()
            .unwrap()
            .extend(upserts.map(|t| (t.path.clone(), t.artist_biography.clone())));
        fs::write(&biography, "Read when needed.").unwrap();
        Ok(())
    });
    scan_flushing_often(store.as_ref(), &lib);

    let added = root.join("A/Y/01 - New.wav");
    write_small_wav(&added);
    let scan = scan_flushing_often(&hooked, &lib);
    assert_eq!((scan.progress.added, scan.progress.updated), (1, 0), "{:?}", scan.progress);
    assert_eq!(*written.lock().unwrap(), [(added, Some("Read when needed.".to_owned()))]);
}

/// A directory's sidecars are read once per scan, however many folders below inherit them.
/// The biography changes as each new track's batch is applied, yet both sibling folders carry
/// the text read for the first.
pub fn sidecars_are_read_once_per_directory<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let biography = root.join("A/artist.txt");
    write_small_wav(&root.join("A/Y/01 - One.wav"));
    write_small_wav(&root.join("A/Z/01 - Two.wav"));
    fs::write(&biography, "Version 0").unwrap();
    let store = Arc::new(make(&library(&root)));
    let applied = AtomicUsize::new(0);
    let texts = Mutex::new(Vec::new());
    let hooked = Hooked::new(store.clone()).on_apply(|batch| {
        texts.lock().unwrap().extend(batch.upserts.iter().map(|t| t.artist_biography.clone()));
        let n = applied.fetch_add(1, Ordering::SeqCst) + 1;
        fs::write(&biography, format!("Version {n}")).unwrap();
        Ok(())
    });
    let lib = library(&root);
    let scan = scan_flushing_often(&hooked, &lib);
    assert_eq!(scan.progress.added, 2, "{:?}", scan.progress);
    let first = Some("Version 0".to_owned());
    assert_eq!(*texts.lock().unwrap(), [first.clone(), first]);
}
