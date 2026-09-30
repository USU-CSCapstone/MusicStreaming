//! The scanner's [`Store`] on SQLite (`design/scanning.md` §7,
//! `design/database.md` §3).
//!
//! Paths are stored relative to their root. Every batch is one transaction
//! that also maintains the derived data the schema assigns to the scanner
//! and appends the net effect to the change feed.

mod entities;
mod jobs;
mod problems;
mod roots;
mod scans;
mod tracks;
mod upsert;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use jewelcase_scanner::store::{Batch, IndexedFile, Store, StoreError};
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
        let store = SqliteStore {
            db,
            roots: Arc::default(),
        };
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

    fn lib(id: &LibraryId) -> Option<i64> {
        id.parse().ok()
    }
}

/// The scanner's calls cannot fail, so an error is logged and the scanner carries on with
/// `default`.
fn logged<T>(what: &str, result: Result<T, DbError>, default: T) -> T {
    result.unwrap_or_else(|error| {
        tracing::error!(%error, "store: {what} failed");
        default
    })
}

impl Store for SqliteStore {
    fn create_scan(&self, scan: &Scan) -> ScanId {
        let Some(lib) = Self::lib(&scan.library) else {
            return 0;
        };
        logged("create_scan", scans::create(self, lib, scan), 0)
    }

    fn update_scan(&self, scan: &Scan) {
        logged("update_scan", scans::update(self, scan), ());
    }

    fn interrupted_scans(&self, library: &LibraryId) -> Vec<Scan> {
        let Some(lib) = Self::lib(library) else {
            return Vec::new();
        };
        logged(
            "interrupted_scans",
            scans::interrupted(self, lib),
            Vec::new(),
        )
    }

    fn lookup(&self, library: &LibraryId, path: &Path) -> Option<IndexedFile> {
        let lib = Self::lib(library)?;
        logged("lookup", tracks::lookup(self, lib, path), None)
    }

    fn apply(&self, library: &LibraryId, batch: Batch) -> Result<(), StoreError> {
        let Some(lib) = Self::lib(library) else {
            return Err(StoreError(format!("unknown library id {library}")));
        };
        tracks::apply(self, lib, batch).map_err(|error| {
            tracing::error!(%error, "store: apply failed");
            StoreError(error.to_string())
        })
    }

    fn mark_missing_unseen(&self, library: &LibraryId, scope: &Scope, scan_id: ScanId) -> usize {
        let Some(lib) = Self::lib(library) else {
            return 0;
        };
        let marked = tracks::mark_missing_unseen(self, lib, scope, scan_id);
        logged("mark_missing_unseen", marked, 0)
    }

    fn next_unanalyzed(
        &self,
        library: &LibraryId,
        analyzer_version: u32,
        limit: usize,
    ) -> Vec<(TrackId, PathBuf)> {
        let Some(lib) = Self::lib(library) else {
            return Vec::new();
        };
        let next = jobs::next_unanalyzed(self, lib, analyzer_version, limit);
        logged("next_unanalyzed", next, Vec::new())
    }

    fn store_analysis(&self, library: &LibraryId, track_id: TrackId, result: AnalysisResult) {
        if let Some(lib) = Self::lib(library) {
            let stored = jobs::store_analysis(self, lib, track_id, result);
            logged("store_analysis", stored, ());
        }
    }

    fn next_without_placeholder(
        &self,
        library: &LibraryId,
        limit: usize,
    ) -> Vec<(Vec<u8>, PathBuf)> {
        let Some(lib) = Self::lib(library) else {
            return Vec::new();
        };
        let next = jobs::next_without_placeholder(self, lib, limit);
        logged("next_without_placeholder", next, Vec::new())
    }

    fn store_placeholder(&self, library: &LibraryId, hash: &[u8], placeholder: Vec<u8>) {
        if let Some(lib) = Self::lib(library) {
            let stored = jobs::store_placeholder(self, lib, hash, placeholder);
            logged("store_placeholder", stored, ());
        }
    }

    fn duplicate_candidates(&self, library: &LibraryId) -> Vec<DuplicateCandidate> {
        let Some(lib) = Self::lib(library) else {
            return Vec::new();
        };
        logged(
            "duplicate_candidates",
            tracks::duplicate_candidates(self, lib),
            Vec::new(),
        )
    }
}
