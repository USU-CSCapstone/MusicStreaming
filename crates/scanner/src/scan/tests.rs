use std::fs;
use std::path::PathBuf;

use super::*;
use crate::store::{FileToDecode, MemoryStore, StoreError};
use crate::testing::{library, make_album};

/// A memory store that runs `on_apply` before applying each batch, so a test can change the
/// world partway through a scan.
struct Hooked<F> {
    inner: MemoryStore,
    on_apply: F,
}

impl<F: Fn() + Send + Sync> Store for Hooked<F> {
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
        self.inner.lookup(library, path)
    }
    fn apply(&self, library: &LibraryId, batch: Batch) -> Result<(), StoreError> {
        (self.on_apply)();
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
    fn duplicate_candidates(&self, library: &LibraryId) -> Vec<DuplicateCandidate> {
        self.inner.duplicate_candidates(library)
    }
}

/// Runs a new scan of the whole library against `store`, flushing whenever the batch holds
/// anything.
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
    let options = ScanOptions { batch_size: 1, ..ScanOptions::default() };
    let ctx = ScanContext {
        store,
        library: lib,
        governor: &Governor::new(),
        cancel: &AtomicBool::new(false),
        options: &options,
    };
    run_scan(&ctx, &mut scan);
    scan
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
    };
    let store = Hooked { inner: MemoryStore::new(), on_apply };
    scan_flushing_often(&store.inner, &lib);

    // The root goes once A/X's touch lands, so B and B/Y are never listed.
    let scan = scan_flushing_often(&store, &lib);
    assert!(!root.exists());
    assert_eq!(scan.progress.missing, 0);
    assert!(store.inner.tracks(&lib.id).iter().all(|t| !t.missing));
    assert_eq!(scan.state, ScanState::Suspended);
}
