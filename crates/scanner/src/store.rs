//! Persistence as the scanner sees it (`design/scanning.md` §1, §7).
//!
//! The scanner never opens SQLite. It hands the store batches and asks it a
//! handful of questions. [`MemoryStore`] is the reference implementation,
//! used by tests and the example; the server's SQLite store implements the
//! same trait.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::types::*;

/// What the store already knows about a path. Enough for the skip check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFile {
    pub track_id: TrackId,
    pub size: u64,
    pub mtime_ms: u64,
    pub missing: bool,
}

/// One transaction's worth of outcomes (`design/scanning.md` §7).
#[derive(Debug, Default, Clone)]
pub struct Batch {
    pub scan_id: ScanId,
    /// New and updated tracks.
    pub upserts: Vec<TrackRecord>,
    /// Unchanged tracks: set `last_seen_scan` so reconcile leaves them alone.
    pub touched: Vec<TrackId>,
    /// Missing tracks whose files are back unchanged: clear the mark and touch.
    pub returned: Vec<TrackId>,
    pub problems: Vec<Problem>,
    /// Paths that scanned successfully: clear any problems recorded for them.
    pub cleared: Vec<PathBuf>,
}

impl Batch {
    pub fn new(scan_id: ScanId) -> Batch {
        Batch {
            scan_id,
            ..Default::default()
        }
    }

    pub fn len(&self) -> usize {
        self.upserts.len() + self.touched.len() + self.returned.len() + self.problems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Everything the scanner needs from persistence. Implementations are
/// synchronous; the scanner calls them from its own blocking threads.
pub trait Store: Send + Sync {
    // --- scans ---
    /// Persist a new scan row and return its id.
    fn create_scan(&self, scan: &Scan) -> ScanId;
    fn update_scan(&self, scan: &Scan);
    /// Scans left in `Running` or `Queued` by a previous process, for resume.
    fn interrupted_scans(&self, library: &LibraryId) -> Vec<Scan>;

    // --- files ---
    fn lookup(&self, library: &LibraryId, path: &Path) -> Option<IndexedFile>;
    /// Apply one batch atomically and append its net effect to the change feed.
    fn apply(&self, library: &LibraryId, batch: Batch);
    /// Mark every non-missing track under `scope` whose `last_seen_scan` is
    /// not `scan_id` as missing. Returns how many.
    fn mark_missing_unseen(&self, library: &LibraryId, scope: &Scope, scan_id: ScanId) -> usize;

    // --- analysis ---
    /// Tracks lacking results at `analyzer_version`, most useful first.
    fn next_unanalyzed(
        &self,
        library: &LibraryId,
        analyzer_version: u32,
        limit: usize,
    ) -> Vec<(TrackId, PathBuf)>;
    fn store_analysis(&self, library: &LibraryId, track_id: TrackId, result: AnalysisResult);

    // --- duplicates ---
    fn duplicate_candidates(&self, library: &LibraryId) -> Vec<DuplicateCandidate>;
}

// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StoredTrack {
    pub record: TrackRecord,
    pub missing: bool,
    pub analysis: Option<AnalysisResult>,
}

#[derive(Default)]
struct LibraryState {
    tracks: BTreeMap<TrackId, StoredTrack>,
    by_path: HashMap<PathBuf, TrackId>,
    problems: HashMap<PathBuf, Problem>,
    scans: BTreeMap<ScanId, Scan>,
    /// Net-effect change feed: one entry per track per batch at most.
    feed: Vec<FeedEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedEntry {
    Added(TrackId),
    Updated(TrackId),
    Missing(TrackId),
    Returned(TrackId),
}

/// In-memory [`Store`]. Complete, so tests exercise every path the SQLite
/// store must support.
#[derive(Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    libraries: HashMap<LibraryId, LibraryState>,
    next_track: TrackId,
    next_scan: ScanId,
}

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }

    pub fn tracks(&self, library: &LibraryId) -> Vec<StoredTrack> {
        let inner = self.inner.lock().unwrap();
        inner
            .libraries
            .get(library)
            .map(|l| l.tracks.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn track(&self, library: &LibraryId, id: TrackId) -> Option<StoredTrack> {
        self.inner
            .lock()
            .unwrap()
            .libraries
            .get(library)
            .and_then(|l| l.tracks.get(&id).cloned())
    }

    pub fn problems(&self, library: &LibraryId) -> Vec<Problem> {
        let inner = self.inner.lock().unwrap();
        inner
            .libraries
            .get(library)
            .map(|l| l.problems.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn scans(&self, library: &LibraryId) -> Vec<Scan> {
        let inner = self.inner.lock().unwrap();
        inner
            .libraries
            .get(library)
            .map(|l| l.scans.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn feed(&self, library: &LibraryId) -> Vec<FeedEntry> {
        let inner = self.inner.lock().unwrap();
        inner
            .libraries
            .get(library)
            .map(|l| l.feed.clone())
            .unwrap_or_default()
    }
}

impl Store for MemoryStore {
    fn create_scan(&self, scan: &Scan) -> ScanId {
        let mut inner = self.inner.lock().unwrap();
        inner.next_scan += 1;
        let id = inner.next_scan;
        let mut scan = scan.clone();
        scan.id = id;
        inner
            .libraries
            .entry(scan.library.clone())
            .or_default()
            .scans
            .insert(id, scan);
        id
    }

    fn update_scan(&self, scan: &Scan) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .libraries
            .entry(scan.library.clone())
            .or_default()
            .scans
            .insert(scan.id, scan.clone());
    }

    fn interrupted_scans(&self, library: &LibraryId) -> Vec<Scan> {
        let inner = self.inner.lock().unwrap();
        inner
            .libraries
            .get(library)
            .map(|l| {
                l.scans
                    .values()
                    .filter(|s| matches!(s.state, ScanState::Running | ScanState::Queued))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn lookup(&self, library: &LibraryId, path: &Path) -> Option<IndexedFile> {
        let inner = self.inner.lock().unwrap();
        let lib = inner.libraries.get(library)?;
        let id = *lib.by_path.get(path)?;
        let t = lib.tracks.get(&id)?;
        Some(IndexedFile {
            track_id: id,
            size: t.record.size,
            mtime_ms: t.record.mtime_ms,
            missing: t.missing,
        })
    }

    fn apply(&self, library: &LibraryId, batch: Batch) {
        let mut inner = self.inner.lock().unwrap();
        let Inner {
            libraries,
            next_track,
            ..
        } = &mut *inner;
        let lib = libraries.entry(library.clone()).or_default();
        for mut record in batch.upserts {
            record.last_seen_scan = batch.scan_id;
            let id = match record.id {
                Some(id) => id,
                None => {
                    *next_track += 1;
                    *next_track
                }
            };
            record.id = Some(id);
            let is_new = !lib.tracks.contains_key(&id);
            // A record replacing the path of another track (identity is path
            // keyed for now) evicts the old path mapping.
            if let Some(old) = lib.tracks.get(&id) {
                lib.by_path.remove(&old.record.path);
            }
            lib.by_path.insert(record.path.clone(), id);
            lib.tracks.insert(
                id,
                StoredTrack {
                    record,
                    missing: false,
                    analysis: None,
                },
            );
            lib.feed.push(if is_new {
                FeedEntry::Added(id)
            } else {
                FeedEntry::Updated(id)
            });
        }
        for id in batch.touched {
            if let Some(t) = lib.tracks.get_mut(&id) {
                t.record.last_seen_scan = batch.scan_id;
            }
        }
        for id in batch.returned {
            if let Some(t) = lib.tracks.get_mut(&id) {
                t.record.last_seen_scan = batch.scan_id;
                if t.missing {
                    t.missing = false;
                    lib.feed.push(FeedEntry::Returned(id));
                }
            }
        }
        for path in batch.cleared {
            lib.problems.remove(&path);
        }
        for problem in batch.problems {
            lib.problems.insert(problem.path.clone(), problem);
        }
    }

    fn mark_missing_unseen(&self, library: &LibraryId, scope: &Scope, scan_id: ScanId) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let Some(lib) = inner.libraries.get_mut(library) else {
            return 0;
        };
        let mut count = 0;
        for (id, t) in lib.tracks.iter_mut() {
            if !t.missing
                && t.record.last_seen_scan != scan_id
                && scope.contains_file(&t.record.path)
            {
                t.missing = true;
                lib.feed.push(FeedEntry::Missing(*id));
                count += 1;
            }
        }
        count
    }

    fn next_unanalyzed(
        &self,
        library: &LibraryId,
        analyzer_version: u32,
        limit: usize,
    ) -> Vec<(TrackId, PathBuf)> {
        let inner = self.inner.lock().unwrap();
        let Some(lib) = inner.libraries.get(library) else {
            return Vec::new();
        };
        lib.tracks
            .iter()
            .filter(|(_, t)| {
                !t.missing
                    && t.analysis
                        .as_ref()
                        .is_none_or(|a| a.analyzer_version < analyzer_version)
            })
            .map(|(id, t)| (*id, t.record.path.clone()))
            .take(limit)
            .collect()
    }

    fn store_analysis(&self, library: &LibraryId, track_id: TrackId, result: AnalysisResult) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(t) = inner
            .libraries
            .get_mut(library)
            .and_then(|l| l.tracks.get_mut(&track_id))
        {
            t.analysis = Some(result);
        }
    }

    fn duplicate_candidates(&self, library: &LibraryId) -> Vec<DuplicateCandidate> {
        let inner = self.inner.lock().unwrap();
        let Some(lib) = inner.libraries.get(library) else {
            return Vec::new();
        };
        lib.tracks
            .iter()
            .filter(|(_, t)| !t.missing)
            .map(|(id, t)| DuplicateCandidate {
                track_id: *id,
                path: t.record.path.clone(),
                title: t.record.tags.title.clone(),
                artists: t.record.tags.artists.clone(),
                album: t.record.tags.album.clone(),
                album_artists: t.record.tags.album_artists.clone(),
                track_number: t.record.tags.track_number,
                disc_number: t.record.tags.disc_number,
            })
            .collect()
    }
}
