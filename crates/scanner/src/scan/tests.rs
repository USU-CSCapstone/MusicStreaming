use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;

use super::*;
use crate::store::{MemoryStore, StoreError};
use crate::testing::{Hooked, library, make_album, write_small_wav};

/// The flags that cancel a scan, or shut it down, from inside it.
#[derive(Default)]
struct Stops {
    cancel: AtomicBool,
    shutdown: AtomicBool,
}

/// A new manual scan of the library's one root, saved to `store`.
fn new_scan(store: &dyn Store, lib: &LibraryConfig) -> Scan {
    let mut scan = Scan {
        id: 0,
        library: lib.id,
        trigger: Trigger::Manual,
        scopes: vec![Scope::root(&lib.roots[0])],
        state: ScanState::Queued,
        started_at: None,
        finished_at: None,
        progress: ScanProgress::default(),
        cursor: None,
    };
    scan.id = store.create_scan(&scan).unwrap();
    scan
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
fn only_row<F, L, W>(store: &Hooked<F, L, W>, lib: &LibraryConfig) -> Scan {
    let [row] = store.inner.scans(lib.id).try_into().unwrap();
    row
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
#[test]
fn a_stop_flushes_the_batch_in_hand() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    make_album(&root, "C", "Z", 1);
    let lib = library(&root);
    let stops = Stops::default();
    let stop_at = root.join("B/Y/01 - Track 1.wav");
    let on_lookup = |path: &Path| {
        if path == stop_at {
            stops.shutdown.store(true, Ordering::SeqCst);
        }
        Ok(())
    };
    let store = Hooked {
        inner: MemoryStore::new(),
        on_apply: || Ok(()),
        on_lookup,
        on_under: || Ok(()),
        on_scan_write: || Ok(()),
    };
    let mut scan = new_scan(&store, &lib);
    run(&store, &lib, &mut scan, &batches_of(1000), &stops);

    let [row] = store.inner.scans(lib.id).try_into().unwrap();
    assert_eq!((row.state, &row.cursor), (ScanState::Running, &after(&root, "B/Y")), "{row:?}");
    assert_eq!(row.finished_at, None);
    let mut paths: Vec<_> = store.inner.tracks(lib.id).into_iter().map(|t| t.record.path).collect();
    paths.sort();
    assert_eq!(paths, [root.join("A/X/01 - Track 1.wav"), stop_at.clone()]);
}

/// A root that goes away after the walk began suspends the scan before reconcile, and nothing
/// the walk did not reach is marked missing (`requirements/scanning.md` §7).
#[test]
fn a_root_gone_before_reconcile_suspends_and_marks_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("nas");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 2);
    let lib = library(&root);
    let away = tmp.path().join("away");
    let on_apply = || {
        if root.exists() {
            fs::rename(&root, &away).unwrap();
        }
        Ok(())
    };
    let store = Hooked {
        inner: MemoryStore::new(),
        on_apply,
        on_lookup: |_: &Path| Ok(()),
        on_under: || Ok(()),
        on_scan_write: || Ok(()),
    };
    scan_flushing_often(&store.inner, &lib);

    // The root goes once A/X's touch lands, so B and B/Y are never listed.
    let scan = scan_flushing_often(&store, &lib);
    assert!(!root.exists());
    assert_eq!(scan.progress.missing, 0);
    assert!(store.inner.tracks(lib.id).iter().all(|t| !t.missing));
    assert_eq!(scan.state, ScanState::Suspended);
}

/// Sidecars are read only for a file past stage 2, so a folder of unchanged files opens none.
/// Here an unchanged track sits beside `artist.txt`, and a new track one folder below inherits
/// it. The biography changes as the unchanged track's touch is applied, so the new track gets
/// the later text only because the unchanged track never read the file.
#[test]
fn an_unchanged_file_reads_no_sidecars() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let biography = root.join("A/artist.txt");
    write_small_wav(&root.join("A/01 - Kept.wav"));
    fs::write(&biography, "Read early.").unwrap();
    let lib = library(&root);
    let on_apply = || {
        fs::write(&biography, "Read when needed.").unwrap();
        Ok(())
    };
    let store = Hooked {
        inner: MemoryStore::new(),
        on_apply,
        on_lookup: |_: &Path| Ok(()),
        on_under: || Ok(()),
        on_scan_write: || Ok(()),
    };
    scan_flushing_often(&store.inner, &lib);

    let added = root.join("A/Y/01 - New.wav");
    write_small_wav(&added);
    let scan = scan_flushing_often(&store, &lib);
    assert_eq!((scan.progress.added, scan.progress.updated), (1, 0), "{:?}", scan.progress);
    let tracks = store.inner.tracks(lib.id);
    let new = tracks.iter().find(|t| t.record.path == added).unwrap();
    let text = new.record.artist_biography.as_ref().map(|b| b.text.as_str());
    assert_eq!(text, Some("Read when needed."));
}

/// A directory's sidecars are read once per scan, however many folders below inherit them.
/// The biography changes as each new track's batch is applied, yet both sibling folders carry
/// the text read for the first.
#[test]
fn sidecars_are_read_once_per_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let biography = root.join("A/artist.txt");
    write_small_wav(&root.join("A/Y/01 - One.wav"));
    write_small_wav(&root.join("A/Z/01 - Two.wav"));
    fs::write(&biography, "Version 0").unwrap();
    let applied = AtomicUsize::new(0);
    let on_apply = || {
        let n = applied.fetch_add(1, Ordering::SeqCst) + 1;
        fs::write(&biography, format!("Version {n}")).unwrap();
        Ok(())
    };
    let store = Hooked {
        inner: MemoryStore::new(),
        on_apply,
        on_lookup: |_: &Path| Ok(()),
        on_under: || Ok(()),
        on_scan_write: || Ok(()),
    };
    let lib = library(&root);
    let scan = scan_flushing_often(&store, &lib);
    assert_eq!(scan.progress.added, 2, "{:?}", scan.progress);
    let texts: Vec<_> = store
        .inner
        .tracks(lib.id)
        .into_iter()
        .map(|t| t.record.artist_biography.map(|b| b.text))
        .collect();
    assert_eq!(texts, [Some("Version 0".to_owned()), Some("Version 0".to_owned())]);
}

mod checkpoints;
mod excludes;
mod failures;
mod panics;
mod resume;
mod undone;
mod unlistable;
