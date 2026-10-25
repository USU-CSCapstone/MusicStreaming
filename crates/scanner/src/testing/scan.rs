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
    Cursor, Scan, ScanContext, ScanOptions, ScanProgress, ScanState, Trigger, run_scan,
};
use crate::scope::Scope;
use crate::store::{Batch, Store, StoreError};
use crate::track::TrackRecord;

mod checkpoints;
mod excludes;
mod failures;
mod panics;
mod reconcile;
mod resume;
pub use checkpoints::*;
pub use excludes::*;
pub use failures::*;
pub use panics::*;
pub use reconcile::*;
pub use resume::*;

/// The flag that shuts a scan down from inside it.
#[derive(Default)]
struct Stops {
    shutdown: AtomicBool,
}

/// A new manual scan of the library's one root, saved to `store`.
fn new_scan(store: &dyn Store, lib: &LibraryConfig) -> Scan {
    store.create_scan(lib.id, Trigger::Manual, &[Scope::root(&lib.roots[0])]).unwrap()
}

/// Batches of `batch_size` outcomes after a first of one, never flushed on a timer or sized by
/// how long they take to apply.
fn batches_of(batch_size: usize) -> ScanOptions {
    ScanOptions {
        batch_size,
        batch_interval: Duration::MAX,
        batch_time: Duration::MAX,
        min_batch: 1,
        ..Default::default()
    }
}

/// Runs `scan`, already saved to `store`, with `options`, shut down once `stops` says so, and
/// returns the scopes whose root did not answer, which a retry may cure.
fn run(
    store: &dyn Store,
    lib: &LibraryConfig,
    scan: &mut Scan,
    options: &ScanOptions,
    stops: &Stops,
) -> Vec<Scope> {
    let ctx = ScanContext { store, library: lib, shutdown: &stops.shutdown, options };
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

/// A stop flushes the batch in hand: a shutdown asked for as the scan starts on C leaves the row
/// running at C with A's and B's tracks stored, though no batch filled and no timer fired, and
/// C/Z never read.
pub fn a_stop_flushes_the_batch_in_hand<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    make_album(&root, "C", "Z", 1);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let stops = Stops::default();
    let stop_at = root.join("C");
    let hooked = Hooked::new(store.clone()).on_folder(|path| {
        if path == stop_at {
            stops.shutdown.store(true, Ordering::SeqCst);
        }
        Ok(())
    });
    let mut scan = new_scan(&hooked, &lib);
    let options = ScanOptions { min_batch: 1000, ..batches_of(1000) };
    run(&hooked, &lib, &mut scan, &options, &stops);

    let row = only_row(store.as_ref(), &lib);
    assert_eq!((row.state, &row.cursor), (ScanState::Running, &after(&root, "C")), "{row:?}");
    assert_eq!(row.finished_at, None);
    assert_eq!(paths(store.as_ref(), &lib), [track(&root, "A/X", 1), track(&root, "B/Y", 1)]);
}

/// A root that goes away during the walk suspends the scan before anything is marked missing
/// (`requirements/scanning.md` §7). Here it goes as the scan starts on A/X, already listed
/// without its deleted second track, so marking that track finds the root gone; it comes back as
/// the batch in hand is flushed, before the check at the scope's end, and the scope is still left
/// for a retry. A suspended scan never resumes, so A/X's one file counts as seen.
pub fn a_root_gone_before_reconcile_suspends_and_marks_nothing<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("nas");
    make_album(&root, "A", "X", 2);
    make_album(&root, "B", "Y", 2);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let away = tmp.path().join("away");
    let gone_at = root.join("A/X");
    let on_folder = |path: &Path| {
        if path == gone_at {
            fs::rename(&root, &away).unwrap();
        }
        Ok(())
    };
    let on_apply = |_: &Batch| {
        if !root.exists() {
            fs::rename(&away, &root).unwrap();
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_folder(on_folder).on_apply(on_apply);
    scan_flushing_often(store.as_ref(), &lib);
    fs::remove_file(track(&root, "A/X", 2)).unwrap();

    let mut scan = new_scan(&hooked, &lib);
    let unanswered = run(&hooked, &lib, &mut scan, &batches_of(1), &Stops::default());
    assert!(root.exists());
    assert_eq!(scan.state, ScanState::Suspended);
    assert_eq!(unanswered, [Scope::root(&root)]);
    assert_eq!((scan.progress.files_seen, scan.progress.missing), (1, 0), "{:?}", scan.progress);
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));
}

/// A scope whose root goes as its walk is flushed ends the scan suspended with the scope returned
/// for a retry, which no resume from the cursor would walk again.
pub fn a_root_gone_after_its_walk_suspends_and_returns_its_scope<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "C", "Z", 2);
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
    let on_apply = |_: &Batch| {
        if walked.load(Ordering::SeqCst) && root.exists() {
            fs::rename(&root, &away).unwrap();
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_apply(on_apply).on_folder(on_folder);
    let mut scan = new_scan(&hooked, &lib);
    let unanswered = run(&hooked, &lib, &mut scan, &batches_of(1000), &Stops::default());

    let row = only_row(store.as_ref(), &lib);
    assert_eq!((row.state, &row.cursor), (ScanState::Suspended, &after(&root, "C/Z")), "{row:?}");
    assert_eq!(unanswered, [Scope::root(&root)]);
}

/// The text of `track`'s biography, if it has one.
fn biography_text(track: &TrackRecord) -> Option<String> {
    track.artist_biography.as_ref().map(|b| b.text.to_string())
}

/// Sidecars are read only for a file past stage 2, so a folder of unchanged files opens none.
/// Here an unchanged track sits beside `artist.txt`, and a new track one folder below inherits
/// it. The biography changes as the scan starts on the new track's folder, after the unchanged
/// track, so the new track, the only one written, gets the later text only because the unchanged
/// track never read the file.
pub fn an_unchanged_file_reads_no_sidecars<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let biography = root.join("A/artist.txt");
    write_small_wav(&root.join("A/01 - Kept.wav"));
    fs::write(&biography, "Read early.").unwrap();
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    let written = Mutex::new(Vec::new());
    let new_folder = root.join("A/Y");
    let on_apply = |batch: &Batch| {
        let upserts = batch.upserts.iter();
        written.lock().unwrap().extend(upserts.map(|t| (t.path.clone(), biography_text(t))));
        Ok(())
    };
    let on_folder = |path: &Path| {
        if path == new_folder {
            fs::write(&biography, "Read when needed.").unwrap();
        }
        Ok(())
    };
    let hooked = Hooked::new(store.clone()).on_apply(on_apply).on_folder(on_folder);
    scan_flushing_often(store.as_ref(), &lib);

    let added = new_folder.join("01 - New.wav");
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
        texts.lock().unwrap().extend(batch.upserts.iter().map(biography_text));
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
