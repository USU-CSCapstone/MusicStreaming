//! A store that holds scans up, for scenarios that queue, cancel or stop work while a scan runs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use crate::store::{Batch, FileToDecode, IndexedFile, PickedTrack, Store, StoreError};
use crate::types::*;

/// Passes every call to the store it wraps. While held, a scan that saves its row as running
/// waits, once the row is written, until [`Parked::release`]: a scan that starts while held
/// waits before it reads anything.
pub struct Parked {
    inner: Arc<dyn Store>,
    held: Mutex<bool>,
    released: Condvar,
}

impl Parked {
    /// Wraps `inner`, not holding scans up.
    pub fn new(inner: Arc<dyn Store>) -> Arc<Parked> {
        Arc::new(Parked { inner, held: Mutex::new(false), released: Condvar::new() })
    }

    /// Holds up each scan as it next saves its row as running.
    pub fn hold(&self) {
        *self.held.lock().unwrap() = true;
    }

    /// Lets every held scan go on.
    pub fn release(&self) {
        *self.held.lock().unwrap() = false;
        self.released.notify_all();
    }
}

impl Store for Parked {
    fn create_scan(&self, scan: &Scan) -> Result<ScanId, StoreError> {
        self.inner.create_scan(scan)
    }
    fn update_scan(&self, scan: &Scan) -> Result<(), StoreError> {
        self.inner.update_scan(scan)?;
        if scan.state == ScanState::Running {
            let held = self.held.lock().unwrap();
            let _released = self.released.wait_while(held, |held| *held).unwrap();
        }
        Ok(())
    }
    fn interrupted_scans(&self, library: LibraryId) -> Result<Vec<Scan>, StoreError> {
        self.inner.interrupted_scans(library)
    }
    fn lookup(&self, library: LibraryId, path: &Path) -> Result<Option<IndexedFile>, StoreError> {
        self.inner.lookup(library, path)
    }
    fn apply(&self, library: LibraryId, batch: Batch) -> Result<(), StoreError> {
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
        self.inner.problems_under(library, scope)
    }
    fn indexes_under(&self, library: LibraryId, folder: &Path) -> Result<bool, StoreError> {
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
