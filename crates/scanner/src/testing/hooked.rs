//! A memory store with hooks, for the scanner's own tests of what a scan or the queue does when
//! the store is slow, fails, or the world changes partway.

use std::path::{Path, PathBuf};

use crate::store::{Batch, FileToDecode, IndexedFile, MemoryStore, PickedTrack, Store, StoreError};
use crate::types::*;

/// A memory store that runs `on_apply` before applying each batch, `on_lookup` with each path it
/// looks up, `on_under` before [`Store::problems_under`] and [`Store::indexes_under`], and
/// `on_scan_write` before it creates or saves a scan row, failing the call if the hook returns an
/// error, so a test can change the world, hold a call up, or fail it, partway through a scan.
pub struct Hooked<F, L, W> {
    pub inner: MemoryStore,
    pub on_apply: F,
    pub on_lookup: L,
    pub on_under: fn() -> Result<(), StoreError>,
    pub on_scan_write: W,
}

impl<F, L, W> Store for Hooked<F, L, W>
where
    F: Fn() -> Result<(), StoreError> + Send + Sync,
    L: Fn(&Path) -> Result<(), StoreError> + Send + Sync,
    W: Fn() -> Result<(), StoreError> + Send + Sync,
{
    fn create_scan(&self, scan: &Scan) -> Result<ScanId, StoreError> {
        (self.on_scan_write)()?;
        self.inner.create_scan(scan)
    }
    fn update_scan(&self, scan: &Scan) -> Result<(), StoreError> {
        (self.on_scan_write)()?;
        self.inner.update_scan(scan)
    }
    fn interrupted_scans(&self, library: LibraryId) -> Result<Vec<Scan>, StoreError> {
        self.inner.interrupted_scans(library)
    }
    fn lookup(&self, library: LibraryId, path: &Path) -> Result<Option<IndexedFile>, StoreError> {
        (self.on_lookup)(path)?;
        self.inner.lookup(library, path)
    }
    fn apply(&self, library: LibraryId, batch: Batch) -> Result<(), StoreError> {
        (self.on_apply)()?;
        self.inner.apply(library, batch)
    }
    fn mark_missing_unseen(
        &self,
        library: LibraryId,
        scope: &Scope,
        scan_id: ScanId,
        unreadable: &[PathBuf],
    ) -> Result<i64, StoreError> {
        self.inner.mark_missing_unseen(library, scope, scan_id, unreadable)
    }
    fn problems_under(
        &self,
        library: LibraryId,
        scope: &Scope,
    ) -> Result<Vec<PathBuf>, StoreError> {
        (self.on_under)()?;
        self.inner.problems_under(library, scope)
    }
    fn indexes_under(&self, library: LibraryId, folder: &Path) -> Result<bool, StoreError> {
        (self.on_under)()?;
        self.inner.indexes_under(library, folder)
    }
    fn next_unanalyzed(
        &self,
        library: LibraryId,
        analyzer_version: u32,
        limit: usize,
    ) -> Result<Vec<(PickedTrack, FileToDecode)>, StoreError> {
        self.inner.next_unanalyzed(library, analyzer_version, limit)
    }
    fn store_analysis(
        &self,
        library: LibraryId,
        track: PickedTrack,
        result: AnalysisResult,
    ) -> Result<(), StoreError> {
        self.inner.store_analysis(library, track, result)
    }
    fn next_without_placeholder(
        &self,
        library: LibraryId,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, FileToDecode)>, StoreError> {
        self.inner.next_without_placeholder(library, limit)
    }
    fn store_placeholder(
        &self,
        library: LibraryId,
        hash: &[u8],
        placeholder: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.inner.store_placeholder(library, hash, placeholder)
    }
}
