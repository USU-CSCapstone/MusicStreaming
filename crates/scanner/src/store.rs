//! Persistence as the scanner sees it (`design/scanning.md` §1, §7).
//!
//! The scanner never opens SQLite. It hands the store batches and asks it a
//! handful of questions. [`MemoryStore`] is the reference implementation,
//! used by tests and the example; the server's SQLite store implements the
//! same trait.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::confine::{not_a_file, without_links};
use crate::problems::UNOPENED;
use crate::types::*;

/// What the store already knows about a path. Enough for the skip check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFile {
    pub track_id: TrackId,
    pub size: u64,
    /// `None` while the store has forgotten it ([`Batch::changed_unread`]), so no file matches.
    pub mtime_ms: Option<u64>,
    pub missing: bool,
    /// Where its lyrics came from, so a lyrics file added or removed beside an
    /// unchanged track is noticed (`requirements/scanning.md` §7).
    pub lyrics: IndexedLyrics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedLyrics {
    None,
    Embedded,
    Sidecar,
}

/// One transaction's worth of outcomes (`design/scanning.md` §7).
#[derive(Debug, Default, Clone)]
pub struct Batch {
    pub scan_id: ScanId,
    /// New and updated tracks.
    pub upserts: Vec<TrackRecord>,
    /// Unchanged tracks: set `last_seen_scan` so reconcile leaves them alone.
    pub touched: Vec<TrackId>,
    /// Missing tracks whose files are back unchanged, or back and unreadable: clear the mark and
    /// touch.
    pub returned: Vec<TrackId>,
    /// Tracks whose files changed but could not be read: forget each one's mtime, so no file
    /// matches it and every scan reads the file until one reads it, and reset its analysis,
    /// which measured other audio.
    pub changed_unread: Vec<TrackId>,
    pub problems: Vec<Problem>,
    /// Paths that scanned or listed successfully, or no longer exist: clear any problems
    /// recorded for them. Applied before `problems`, so a path in both keeps the problem this
    /// batch records.
    pub cleared: Vec<PathBuf>,
    /// Paths a background job has just opened: clear a problem saying they could not be
    /// ([`crate::problems::UNOPENED`]), and leave any other.
    pub opened: Vec<PathBuf>,
}

impl Batch {
    pub fn new(scan_id: ScanId) -> Batch {
        Batch { scan_id, ..Default::default() }
    }

    pub fn len(&self) -> usize {
        self.upserts.len() + self.touched.len() + self.returned.len() + self.problems.len()
    }

    /// Whether applying the batch would change nothing. Paths to clear or mark opened and tracks
    /// changed but unread count, though not toward [`Batch::len`], which sizes a batch by the
    /// tracks and problems it writes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
            && self.cleared.is_empty()
            && self.opened.is_empty()
            && self.changed_unread.is_empty()
    }
}

/// A persistence failure. A scan that hits one suspends at its last persisted cursor, and a
/// background job backs off as [`crate::jobs`] describes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("store: {0}")]
pub struct StoreError(pub String);

/// A track the analysis queue handed out, with its file's size and mtime when it was picked,
/// which [`Store::store_analysis`] checks before storing its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickedTrack {
    pub id: TrackId,
    pub size: u64,
    pub mtime_ms: u64,
}

/// A file a background job decodes, as the store recorded it. A job reaches it only through
/// [`FileToDecode::open`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileToDecode {
    /// The library root the file lies under.
    pub root: PathBuf,
    /// The file's path, under `root`.
    pub path: PathBuf,
}

impl FileToDecode {
    /// The real path to decode, once the file opens. Refused when a symlink sits below the root
    /// or the file is not a regular one ([`crate::confine`]), and an error when the file is gone,
    /// out of reach, or not permitted.
    pub fn open(&self) -> io::Result<PathBuf> {
        let real = without_links(&self.root, &self.path)?;
        if !fs::metadata(&real)?.is_file() {
            return Err(not_a_file());
        }
        // Unlike a stat, an open fails on a file that may not be read, and NFS revalidates it.
        File::open(&real)?;
        Ok(real)
    }
}

/// Everything the scanner needs from persistence. Implementations are
/// synchronous; the scanner calls them from its own blocking threads. Every
/// method can fail with a [`StoreError`], which the caller handles, so a
/// failure is never mistaken for an answer.
pub trait Store: Send + Sync {
    // --- scans ---
    /// Persist a new scan row and return its id.
    fn create_scan(&self, scan: &Scan) -> Result<ScanId, StoreError>;
    /// Save `scan`'s state, cursor, progress and times over its row.
    fn update_scan(&self, scan: &Scan) -> Result<(), StoreError>;
    /// Scans left in `Running` or `Queued` by a previous process, for resume. A row that cannot
    /// be read is left out, so it does not stop the others resuming, and marked suspended, since
    /// it can never resume.
    fn interrupted_scans(&self, library: LibraryId) -> Result<Vec<Scan>, StoreError>;

    // --- files ---
    /// What the index holds for the file at `path`, or `None` when it holds no track there.
    fn lookup(&self, library: LibraryId, path: &Path) -> Result<Option<IndexedFile>, StoreError>;
    /// Apply one batch atomically and append its net effect to the change feed.
    /// An upserted track keeps its analysis only while its file's size and mtime
    /// are unchanged. An error means nothing in the batch was persisted.
    fn apply(&self, library: LibraryId, batch: Batch) -> Result<(), StoreError>;
    /// Mark every non-missing track under `scope` whose `last_seen_scan` is
    /// not `scan_id` as missing, except those at or under a path in
    /// `unreadable`, which the scan could not read. Returns how many.
    fn mark_missing_unseen(
        &self,
        library: LibraryId,
        scope: &Scope,
        scan_id: ScanId,
        unreadable: &[PathBuf],
    ) -> Result<i64, StoreError>;
    /// The paths with a recorded problem that `scope` reaches: under it, or for a Directory
    /// scope directly in it ([`Scope::contains_file`]).
    fn problems_under(&self, library: LibraryId, scope: &Scope)
    -> Result<Vec<PathBuf>, StoreError>;
    /// Whether a track not yet missing, or a problem, lies under `folder`, at any depth.
    fn indexes_under(&self, library: LibraryId, folder: &Path) -> Result<bool, StoreError>;

    // --- analysis ---
    /// Tracks whose results come from an analyzer older than `analyzer_version`, most useful
    /// first, each with its file. A track never analyzed is at version 0. A track whose mtime is
    /// forgotten waits for a scan to read its file, so analysis does not open the file and clear
    /// its problem first.
    fn next_unanalyzed(
        &self,
        library: LibraryId,
        analyzer_version: u32,
        limit: usize,
    ) -> Result<Vec<(PickedTrack, FileToDecode)>, StoreError>;
    /// Stores `result` on `track`, its loudness and waveform together, unless its file's size or
    /// mtime is no longer the one it was picked with.
    fn store_analysis(
        &self,
        library: LibraryId,
        track: PickedTrack,
        result: AnalysisResult,
    ) -> Result<(), StoreError>;

    // --- placeholders ---
    /// Images with no placeholder yet, by content hash, each with a file to decode it from.
    fn next_without_placeholder(
        &self,
        library: LibraryId,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, FileToDecode)>, StoreError>;
    /// Stores the placeholder of the image with content `hash`. Empty means it could not be
    /// decoded, so it is not tried again until its content changes.
    fn store_placeholder(
        &self,
        library: LibraryId,
        hash: &[u8],
        placeholder: Vec<u8>,
    ) -> Result<(), StoreError>;
}

// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StoredTrack {
    pub record: TrackRecord,
    pub missing: bool,
    /// Whether its mtime is forgotten ([`Batch::changed_unread`]).
    pub mtime_unknown: bool,
    pub analysis: Option<AnalysisResult>,
}

#[derive(Default)]
struct LibraryState {
    tracks: BTreeMap<TrackId, StoredTrack>,
    by_path: HashMap<PathBuf, TrackId>,
    problems: HashMap<PathBuf, Problem>,
    scans: BTreeMap<ScanId, Scan>,
    /// Placeholders by image content hash.
    placeholders: HashMap<Vec<u8>, Vec<u8>>,
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

    /// Every track of `library`, missing or not.
    pub fn tracks(&self, library: LibraryId) -> Vec<StoredTrack> {
        let inner = self.inner.lock().unwrap();
        inner
            .libraries
            .get(&library)
            .map(|l| l.tracks.values().cloned().collect())
            .unwrap_or_default()
    }

    /// The track `id` of `library`, if there is one.
    pub fn track(&self, library: LibraryId, id: TrackId) -> Option<StoredTrack> {
        self.inner.lock().unwrap().libraries.get(&library).and_then(|l| l.tracks.get(&id).cloned())
    }

    /// Every problem recorded for `library`.
    pub fn problems(&self, library: LibraryId) -> Vec<Problem> {
        let inner = self.inner.lock().unwrap();
        inner
            .libraries
            .get(&library)
            .map(|l| l.problems.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Every scan of `library`, oldest first.
    pub fn scans(&self, library: LibraryId) -> Vec<Scan> {
        let inner = self.inner.lock().unwrap();
        inner
            .libraries
            .get(&library)
            .map(|l| l.scans.values().cloned().collect())
            .unwrap_or_default()
    }

    /// The placeholder stored for the image with content `hash`.
    pub fn placeholder(&self, library: LibraryId, hash: &[u8]) -> Option<Vec<u8>> {
        let inner = self.inner.lock().unwrap();
        inner.libraries.get(&library)?.placeholders.get(hash).cloned()
    }

    /// `library`'s change feed, oldest entry first.
    pub fn feed(&self, library: LibraryId) -> Vec<FeedEntry> {
        let inner = self.inner.lock().unwrap();
        inner.libraries.get(&library).map(|l| l.feed.clone()).unwrap_or_default()
    }
}

impl Store for MemoryStore {
    fn create_scan(&self, scan: &Scan) -> Result<ScanId, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.next_scan += 1;
        let id = inner.next_scan;
        let mut scan = scan.clone();
        scan.id = id;
        inner.libraries.entry(scan.library).or_default().scans.insert(id, scan);
        Ok(id)
    }

    fn update_scan(&self, scan: &Scan) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.libraries.entry(scan.library).or_default().scans.insert(scan.id, scan.clone());
        Ok(())
    }

    fn interrupted_scans(&self, library: LibraryId) -> Result<Vec<Scan>, StoreError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .libraries
            .get(&library)
            .map(|l| {
                l.scans
                    .values()
                    .filter(|s| matches!(s.state, ScanState::Running | ScanState::Queued))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    fn lookup(&self, library: LibraryId, path: &Path) -> Result<Option<IndexedFile>, StoreError> {
        let inner = self.inner.lock().unwrap();
        let found = inner.libraries.get(&library).and_then(|lib| {
            let id = *lib.by_path.get(path)?;
            Some((id, lib.tracks.get(&id)?))
        });
        Ok(found.map(|(id, t)| IndexedFile {
            track_id: id,
            size: t.record.size,
            mtime_ms: (!t.mtime_unknown).then_some(t.record.mtime_ms),
            missing: t.missing,
            lyrics: if t.record.tags.lyrics.is_some() {
                IndexedLyrics::Embedded
            } else if t.record.lyrics_sidecar.is_some() {
                IndexedLyrics::Sidecar
            } else {
                IndexedLyrics::None
            },
        }))
    }

    fn apply(&self, library: LibraryId, batch: Batch) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let Inner { libraries, next_track, .. } = &mut *inner;
        let lib = libraries.entry(library).or_default();
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
            let old = lib.tracks.remove(&id);
            let is_new = old.is_none();
            // A record replacing the path of another track (identity is path
            // keyed for now) evicts the old path mapping.
            if let Some(old) = &old {
                lib.by_path.remove(&old.record.path);
            }
            let analysis = old
                .filter(|old| {
                    old.record.size == record.size && old.record.mtime_ms == record.mtime_ms
                })
                .and_then(|old| old.analysis);
            lib.by_path.insert(record.path.clone(), id);
            let stored = StoredTrack { record, missing: false, mtime_unknown: false, analysis };
            lib.tracks.insert(id, stored);
            lib.feed.push(if is_new { FeedEntry::Added(id) } else { FeedEntry::Updated(id) });
        }
        for id in batch.touched {
            if let Some(t) = lib.tracks.get_mut(&id) {
                t.record.last_seen_scan = batch.scan_id;
            }
        }
        for &id in &batch.returned {
            if let Some(t) = lib.tracks.get_mut(&id) {
                t.record.last_seen_scan = batch.scan_id;
                if t.missing {
                    t.missing = false;
                    lib.feed.push(FeedEntry::Returned(id));
                }
            }
        }
        for id in batch.changed_unread {
            if let Some(t) = lib.tracks.get_mut(&id)
                && !t.mtime_unknown
            {
                t.mtime_unknown = true;
                t.analysis = None;
                // A track this batch returned has its one feed entry already.
                if !batch.returned.contains(&id) {
                    lib.feed.push(FeedEntry::Updated(id));
                }
            }
        }
        for path in batch.cleared {
            lib.problems.remove(&path);
        }
        for path in batch.opened {
            if lib.problems.get(&path).is_some_and(|p| UNOPENED.contains(&p.kind)) {
                lib.problems.remove(&path);
            }
        }
        for problem in batch.problems {
            lib.problems.insert(problem.path.clone(), problem);
        }
        Ok(())
    }

    fn mark_missing_unseen(
        &self,
        library: LibraryId,
        scope: &Scope,
        scan_id: ScanId,
        unreadable: &[PathBuf],
    ) -> Result<i64, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let Some(lib) = inner.libraries.get_mut(&library) else {
            return Ok(0);
        };
        let unreadable: HashSet<&Path> = unreadable.iter().map(PathBuf::as_path).collect();
        let mut count = 0;
        for (id, t) in lib.tracks.iter_mut() {
            if !t.missing
                && t.record.last_seen_scan != scan_id
                && scope.contains_file(&t.record.path)
                && !t.record.path.ancestors().any(|a| unreadable.contains(a))
            {
                t.missing = true;
                lib.feed.push(FeedEntry::Missing(*id));
                count += 1;
            }
        }
        Ok(count)
    }

    fn problems_under(
        &self,
        library: LibraryId,
        scope: &Scope,
    ) -> Result<Vec<PathBuf>, StoreError> {
        let inner = self.inner.lock().unwrap();
        let Some(lib) = inner.libraries.get(&library) else {
            return Ok(Vec::new());
        };
        Ok(lib.problems.keys().filter(|path| scope.contains_file(path)).cloned().collect())
    }

    fn indexes_under(&self, library: LibraryId, folder: &Path) -> Result<bool, StoreError> {
        let inner = self.inner.lock().unwrap();
        let Some(lib) = inner.libraries.get(&library) else {
            return Ok(false);
        };
        let under = |path: &Path| path != folder && path.starts_with(folder);
        Ok(lib.tracks.values().any(|t| !t.missing && under(&t.record.path))
            || lib.problems.keys().any(|path| under(path)))
    }

    fn next_unanalyzed(
        &self,
        library: LibraryId,
        analyzer_version: u32,
        limit: usize,
    ) -> Result<Vec<(PickedTrack, FileToDecode)>, StoreError> {
        let inner = self.inner.lock().unwrap();
        let Some(lib) = inner.libraries.get(&library) else {
            return Ok(Vec::new());
        };
        Ok(lib
            .tracks
            .iter()
            .filter(|(_, t)| {
                !t.missing
                    && !t.mtime_unknown
                    && t.analysis.as_ref().map_or(0, |a| a.analyzer_version) < analyzer_version
            })
            .map(|(&id, t)| {
                let (size, mtime_ms) = (t.record.size, t.record.mtime_ms);
                let file =
                    FileToDecode { root: t.record.root.clone(), path: t.record.path.clone() };
                (PickedTrack { id, size, mtime_ms }, file)
            })
            .take(limit)
            .collect())
    }

    fn store_analysis(
        &self,
        library: LibraryId,
        track: PickedTrack,
        result: AnalysisResult,
    ) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        let stored = inner.libraries.get_mut(&library).and_then(|l| l.tracks.get_mut(&track.id));
        if let Some(t) = stored
            && !t.mtime_unknown
            && t.record.size == track.size
            && t.record.mtime_ms == track.mtime_ms
        {
            t.analysis = Some(result);
        }
        Ok(())
    }

    fn next_without_placeholder(
        &self,
        library: LibraryId,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, FileToDecode)>, StoreError> {
        let inner = self.inner.lock().unwrap();
        let Some(lib) = inner.libraries.get(&library) else {
            return Ok(Vec::new());
        };
        let mut seen = HashSet::new();
        Ok(lib
            .tracks
            .values()
            .flat_map(|t| {
                let artwork = t.record.artwork.as_ref().map(|art| match &art.source {
                    ArtworkSource::Embedded => (&art.info.hash, &t.record.path),
                    ArtworkSource::Sidecar(path) => (&art.info.hash, path),
                });
                let artist =
                    t.record.artist_image.as_ref().map(|image| (&image.info.hash, &image.path));
                artwork.into_iter().chain(artist).map(|(hash, path)| (hash, &t.record.root, path))
            })
            .filter(|(hash, ..)| !lib.placeholders.contains_key(*hash) && seen.insert(*hash))
            .map(|(hash, root, path)| {
                (hash.clone(), FileToDecode { root: root.clone(), path: path.clone() })
            })
            .take(limit)
            .collect())
    }

    fn store_placeholder(
        &self,
        library: LibraryId,
        hash: &[u8],
        placeholder: Vec<u8>,
    ) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(lib) = inner.libraries.get_mut(&library) {
            lib.placeholders.insert(hash.to_vec(), placeholder);
        }
        Ok(())
    }
}
