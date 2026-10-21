use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;

use super::*;
use crate::store::{FileToDecode, IndexedFile, MemoryStore, StoreError};
use crate::testing::{library, make_album, write_small_wav};

/// A memory store that runs `on_apply` before applying each batch, failing the batch if it
/// returns an error, and `on_lookup` with each path it looks up, so a test can change the world,
/// or fail, partway through a scan.
struct Hooked<F, L> {
    inner: MemoryStore,
    on_apply: F,
    on_lookup: L,
}

impl<F, L> Store for Hooked<F, L>
where
    F: Fn() -> Result<(), StoreError> + Send + Sync,
    L: Fn(&Path) + Send + Sync,
{
    fn create_scan(&self, scan: &Scan) -> ScanId {
        self.inner.create_scan(scan)
    }
    fn update_scan(&self, scan: &Scan) {
        self.inner.update_scan(scan)
    }
    fn interrupted_scans(&self, library: &LibraryId) -> Vec<Scan> {
        self.inner.interrupted_scans(library)
    }
    fn lookup(&self, library: &LibraryId, path: &Path) -> Option<IndexedFile> {
        (self.on_lookup)(path);
        self.inner.lookup(library, path)
    }
    fn apply(&self, library: &LibraryId, batch: Batch) -> Result<(), StoreError> {
        (self.on_apply)()?;
        self.inner.apply(library, batch)
    }
    fn mark_missing_unseen(
        &self,
        library: &LibraryId,
        scope: &Scope,
        scan_id: ScanId,
        unreadable: &[PathBuf],
    ) -> usize {
        self.inner.mark_missing_unseen(library, scope, scan_id, unreadable)
    }
    fn problems_under(&self, library: &LibraryId, scope: &Scope) -> Vec<PathBuf> {
        self.inner.problems_under(library, scope)
    }
    fn indexes_under(&self, library: &LibraryId, folder: &Path) -> bool {
        self.inner.indexes_under(library, folder)
    }
    fn next_unanalyzed(
        &self,
        library: &LibraryId,
        analyzer_version: u32,
        limit: usize,
    ) -> Vec<(TrackId, FileToDecode)> {
        self.inner.next_unanalyzed(library, analyzer_version, limit)
    }
    fn store_analysis(&self, library: &LibraryId, track_id: TrackId, result: AnalysisResult) {
        self.inner.store_analysis(library, track_id, result)
    }
    fn next_without_placeholder(
        &self,
        library: &LibraryId,
        limit: usize,
    ) -> Vec<(Vec<u8>, FileToDecode)> {
        self.inner.next_without_placeholder(library, limit)
    }
    fn store_placeholder(&self, library: &LibraryId, hash: &[u8], placeholder: Vec<u8>) {
        self.inner.store_placeholder(library, hash, placeholder)
    }
}

/// Runs a new scan of the whole library against `store`, flushing whenever the batch holds
/// a track or problem, and never on a timer.
fn scan_flushing_often(store: &dyn Store, lib: &LibraryConfig) -> Scan {
    let mut scan = Scan {
        id: 0,
        library: lib.id.clone(),
        trigger: Trigger::Manual,
        scopes: vec![Scope::root(&lib.roots[0])],
        state: ScanState::Queued,
        started_at: None,
        finished_at: None,
        progress: ScanProgress::default(),
        cursor: None,
    };
    scan.id = store.create_scan(&scan);
    let options = ScanOptions { batch_size: 1, batch_interval: Duration::MAX };
    let ctx = ScanContext {
        store,
        library: lib,
        governor: &Governor::new(),
        cancel: &AtomicBool::new(false),
        shutdown: &AtomicBool::new(false),
        options: &options,
    };
    run_scan(&ctx, &mut scan);
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
    let shutdown = AtomicBool::new(false);
    let stop_at = root.join("B/Y/01 - Track 1.wav");
    let on_lookup = |path: &Path| {
        if path == stop_at {
            shutdown.store(true, Ordering::SeqCst);
        }
    };
    let store = Hooked { inner: MemoryStore::new(), on_apply: || Ok(()), on_lookup };
    let mut scan = Scan {
        id: 0,
        library: lib.id.clone(),
        trigger: Trigger::Manual,
        scopes: vec![Scope::root(&root)],
        state: ScanState::Queued,
        started_at: None,
        finished_at: None,
        progress: ScanProgress::default(),
        cursor: None,
    };
    scan.id = store.create_scan(&scan);
    let ctx = ScanContext {
        store: &store,
        library: &lib,
        governor: &Governor::new(),
        cancel: &AtomicBool::new(false),
        shutdown: &shutdown,
        options: &ScanOptions { batch_size: 1000, batch_interval: Duration::MAX },
    };
    run_scan(&ctx, &mut scan);

    let [row] = store.inner.scans(&lib.id).try_into().unwrap();
    let cursor = Some(Cursor { scope_index: 0, after_directory: Some(root.join("B/Y")) });
    assert_eq!((row.state, &row.cursor), (ScanState::Running, &cursor), "{row:?}");
    assert_eq!(row.finished_at, None);
    let mut paths: Vec<_> =
        store.inner.tracks(&lib.id).into_iter().map(|t| t.record.path).collect();
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
    let store = Hooked { inner: MemoryStore::new(), on_apply, on_lookup: |_: &Path| {} };
    scan_flushing_often(&store.inner, &lib);

    // The root goes once A/X's touch lands, so B and B/Y are never listed.
    let scan = scan_flushing_often(&store, &lib);
    assert!(!root.exists());
    assert_eq!(scan.progress.missing, 0);
    assert!(store.inner.tracks(&lib.id).iter().all(|t| !t.missing));
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
    let store = Hooked { inner: MemoryStore::new(), on_apply, on_lookup: |_: &Path| {} };
    scan_flushing_often(&store.inner, &lib);

    let added = root.join("A/Y/01 - New.wav");
    write_small_wav(&added);
    let scan = scan_flushing_often(&store, &lib);
    assert_eq!((scan.progress.added, scan.progress.updated), (1, 0), "{:?}", scan.progress);
    let tracks = store.inner.tracks(&lib.id);
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
    let store = Hooked { inner: MemoryStore::new(), on_apply, on_lookup: |_: &Path| {} };
    let lib = library(&root);
    let scan = scan_flushing_often(&store, &lib);
    assert_eq!(scan.progress.added, 2, "{:?}", scan.progress);
    let texts: Vec<_> = store
        .inner
        .tracks(&lib.id)
        .into_iter()
        .map(|t| t.record.artist_biography.map(|b| b.text))
        .collect();
    assert_eq!(texts, [Some("Version 0".to_owned()), Some("Version 0".to_owned())]);
}

mod panics;
