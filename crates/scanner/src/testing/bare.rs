//! A store that keeps scan rows and indexes nothing.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::analysis::AnalysisResult;
use crate::library::LibraryId;
use crate::problems::Problem;
use crate::scan::{Scan, ScanId, Trigger};
use crate::scope::Scope;
use crate::store::{
    AnalysisStore, Batch, FileToDecode, IndexedFolder, PickedTrack, PlaceholderStore, ProblemLog,
    Store, StoreError,
};

/// A store for tests that need one but not an index: those of the queue, the watcher and the
/// background jobs, and a scan that must find every file new. It keeps scan rows, numbered from 1
/// in the order they are created, and the problems recorded through [`ProblemLog`]. It holds no
/// track, so every file a scan meets is new, every batch is dropped, and the background jobs find
/// nothing to decode.
///
/// It is not a store to check the pipeline against, and it does not keep [`ProblemLog`]'s
/// contract: `record_problem` keeps every problem, even a second one for the same path, and
/// `clear_unopened` does nothing.
#[derive(Default)]
pub struct BareStore {
    scans: Mutex<Vec<Scan>>,
    problems: Mutex<Vec<Problem>>,
}

impl BareStore {
    /// Every scan row, oldest first.
    pub fn scans(&self) -> Vec<Scan> {
        self.scans.lock().unwrap().clone()
    }

    /// Every problem recorded through [`ProblemLog::record_problem`], oldest first.
    pub fn problems(&self) -> Vec<Problem> {
        self.problems.lock().unwrap().clone()
    }
}

impl Store for BareStore {
    /// A queued scan, numbered one past the last.
    fn create_scan(
        &self,
        library: LibraryId,
        trigger: Trigger,
        scopes: &[Scope],
    ) -> Result<Scan, StoreError> {
        let mut scans = self.scans.lock().unwrap();
        let id = ScanId::try_from(scans.len()).unwrap() + 1;
        let scan = Scan::queued(id, library, trigger, scopes.to_vec());
        scans.push(scan.clone());
        Ok(scan)
    }

    /// Replaces the row with `scan`'s id, failing if there is none.
    fn update_scan(&self, scan: &Scan) -> Result<(), StoreError> {
        let mut scans = self.scans.lock().unwrap();
        let row = scans.iter_mut().find(|row| row.id == scan.id);
        *row.ok_or_else(|| StoreError(format!("no scan {}", scan.id)))? = scan.clone();
        Ok(())
    }

    /// None: the rows it keeps are never resumed.
    fn interrupted_scans(&self, _: LibraryId) -> Result<Vec<Scan>, StoreError> {
        Ok(Vec::new())
    }

    /// Nothing: it holds no track.
    fn indexed_in(&self, _: LibraryId, _: &Path) -> Result<IndexedFolder, StoreError> {
        Ok(IndexedFolder::default())
    }

    /// Drops the batch, marking nothing missing.
    fn apply(&self, _: LibraryId, _: Batch) -> Result<i64, StoreError> {
        Ok(0)
    }

    /// None, though it keeps the problems recorded through [`ProblemLog`].
    fn problems_under(&self, _: LibraryId, _: &Scope) -> Result<Vec<PathBuf>, StoreError> {
        Ok(Vec::new())
    }

    /// No, though it keeps the problems recorded through [`ProblemLog`].
    fn indexes_under(&self, _: LibraryId, _: &Path) -> Result<bool, StoreError> {
        Ok(false)
    }
}

impl ProblemLog for BareStore {
    /// Keeps `problem`, beside any already recorded for its path.
    fn record_problem(&self, _: LibraryId, problem: Problem) -> Result<(), StoreError> {
        self.problems.lock().unwrap().push(problem);
        Ok(())
    }

    /// Nothing: it clears no problem.
    fn clear_unopened(&self, _: LibraryId, _: &Path) -> Result<(), StoreError> {
        Ok(())
    }
}

impl AnalysisStore for BareStore {
    /// None: it holds no track.
    fn next_unanalyzed(
        &self,
        _: LibraryId,
        _: u32,
        _: usize,
    ) -> Result<Vec<(PickedTrack, FileToDecode)>, StoreError> {
        Ok(Vec::new())
    }

    /// Drops the result.
    fn store_analysis(
        &self,
        _: LibraryId,
        _: PickedTrack,
        _: AnalysisResult,
    ) -> Result<(), StoreError> {
        Ok(())
    }
}

impl PlaceholderStore for BareStore {
    /// None: it holds no image.
    fn next_without_placeholder(
        &self,
        _: LibraryId,
        _: usize,
    ) -> Result<Vec<(Vec<u8>, FileToDecode)>, StoreError> {
        Ok(Vec::new())
    }

    /// Drops the placeholder.
    fn store_placeholder(&self, _: LibraryId, _: &[u8], _: Vec<u8>) -> Result<(), StoreError> {
        Ok(())
    }
}
