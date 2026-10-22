//! The scanner's [`Store`], and the narrower traits its background jobs use, on
//! SQLite (`design/scanning.md` §7, `design/database.md` §3).
//!
//! Paths are stored relative to their root. Every batch is one transaction
//! that also maintains the derived data the schema assigns to the scanner
//! and appends the net effect to the change feed.

mod entities;
mod folders;
mod jobs;
pub(super) mod names;
mod problems;
mod roots;
mod scans;
mod tracks;
mod upsert;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use jewelcase_scanner::*;

use self::roots::Roots;
use super::{Database, DbError, libraries};

pub struct SqliteStore {
    db: Arc<Database>,
    /// Active roots per library, refreshed on reconfiguration. Shared with the database jobs
    /// that need them.
    roots: Arc<Roots>,
}

impl SqliteStore {
    /// Loads the roots with a blocking read, so call it outside the async runtime.
    pub fn new(db: Arc<Database>) -> Result<SqliteStore, DbError> {
        let store = SqliteStore { db, roots: Arc::default() };
        store.refresh_roots()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn db(&self) -> &Arc<Database> {
        &self.db
    }

    /// Reload roots from `library_roots`. Call after roots change.
    pub fn refresh_roots(&self) -> Result<(), DbError> {
        let map = self
            .db
            .read_blocking(libraries::all)?
            .into_iter()
            .map(|lib| (lib.id, lib.roots))
            .collect();
        *self.roots.0.write().unwrap() = map;
        Ok(())
    }
}

/// A database error, as the scanner sees it.
impl From<DbError> for StoreError {
    fn from(error: DbError) -> StoreError {
        StoreError(error.to_string())
    }
}

impl Store for SqliteStore {
    fn create_scan(
        &self,
        library: LibraryId,
        trigger: Trigger,
        scopes: &[Scope],
    ) -> Result<Scan, StoreError> {
        Ok(scans::create(self, library, trigger, scopes)?)
    }

    fn update_scan(&self, scan: &Scan) -> Result<(), StoreError> {
        Ok(scans::update(self, scan)?)
    }

    fn interrupted_scans(&self, library: LibraryId) -> Result<Vec<Scan>, StoreError> {
        Ok(scans::interrupted(self, library)?)
    }

    fn indexed_in(&self, library: LibraryId, dir: &Path) -> Result<IndexedFolder, StoreError> {
        Ok(folders::indexed_in(self, library, dir)?)
    }

    fn apply(&self, library: LibraryId, batch: Batch) -> Result<i64, StoreError> {
        Ok(tracks::apply(self, library, batch)?)
    }

    fn indexes_under(&self, library: LibraryId, folder: &Path) -> Result<bool, StoreError> {
        Ok(tracks::indexes_under(self, library, folder)?)
    }

    fn problems_under(
        &self,
        library: LibraryId,
        scope: &Scope,
    ) -> Result<Vec<PathBuf>, StoreError> {
        Ok(problems::under(self, library, scope)?)
    }
}

impl ProblemLog for SqliteStore {
    fn record_problem(&self, library: LibraryId, problem: Problem) -> Result<(), StoreError> {
        Ok(problems::record(self, library, problem)?)
    }

    fn clear_unopened(&self, library: LibraryId, path: &Path) -> Result<(), StoreError> {
        Ok(problems::clear_unopened(self, library, path)?)
    }
}

impl AnalysisStore for SqliteStore {
    fn next_unanalyzed(
        &self,
        library: LibraryId,
        analyzer_version: u32,
        limit: usize,
    ) -> Result<Vec<(PickedTrack, FileToDecode)>, StoreError> {
        Ok(jobs::next_unanalyzed(self, library, analyzer_version, limit)?)
    }

    fn store_analysis(
        &self,
        library: LibraryId,
        track: PickedTrack,
        result: AnalysisResult,
    ) -> Result<(), StoreError> {
        Ok(jobs::store_analysis(self, library, track, result)?)
    }
}

impl PlaceholderStore for SqliteStore {
    fn next_without_placeholder(
        &self,
        library: LibraryId,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, FileToDecode)>, StoreError> {
        Ok(jobs::next_without_placeholder(self, library, limit)?)
    }

    fn store_placeholder(
        &self,
        library: LibraryId,
        hash: &[u8],
        placeholder: Vec<u8>,
    ) -> Result<(), StoreError> {
        Ok(jobs::store_placeholder(self, library, hash, placeholder)?)
    }
}
