//! Persistence as the scanner sees it (`design/scanning.md` §1, §7).
//!
//! The scanner never opens SQLite. A scan hands the [`Store`] batches and asks it a handful of
//! questions. Analysis and placeholders are each given a narrower trait, [`AnalysisStore`] or
//! [`PlaceholderStore`], so a background job reaches only its own queue and results and the
//! [`ProblemLog`] it records problems in, and none of a scan's batches or questions. The server's
//! SQLite store implements them all.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use crate::analysis::AnalysisResult;
use crate::confine::{not_a_file, without_links};
use crate::library::LibraryId;
use crate::problems::Problem;
use crate::scan::{Scan, Trigger};
use crate::scope::Scope;
use crate::sidecar::SidecarStamp;
use crate::track::{TrackId, TrackRecord};

/// What the store already knows about a file. Enough for the skip check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFile {
    pub track_id: TrackId,
    pub size: u64,
    /// `None` while the store has forgotten it ([`Batch::changed_unread`]), so no file matches.
    pub mtime_ms: Option<u64>,
    pub missing: bool,
    /// The stamp of the sidecars it resolved when last read, so a sidecar added, changed or
    /// removed beside an unchanged file is noticed (`requirements/scanning.md` §7).
    pub sidecar_stamp: SidecarStamp,
}

/// What the index holds directly in one folder ([`Store::indexed_in`]), by name as the store
/// keeps it: UTF-8, with any invalid sequence replaced, as [`std::ffi::OsStr::to_string_lossy`]
/// gives it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IndexedFolder {
    /// The tracks whose files are directly in the folder, missing or not, by file name.
    pub files: HashMap<String, IndexedFile>,
    /// The name of each folder directly in it under which a track lies, at any depth.
    pub folders: Vec<String>,
}

impl IndexedFolder {
    /// Removes and returns what the index holds for the file at `path`, which lies directly in
    /// the folder, so [`IndexedFolder::files`] keeps only the tracks no listed file has taken.
    pub fn take(&mut self, path: &Path) -> Option<IndexedFile> {
        self.files.remove(path.file_name()?.to_string_lossy().as_ref())
    }
}

/// One transaction's worth of outcomes (`design/scanning.md` §7). An unchanged file is in none
/// of them, so a folder of unchanged files writes nothing.
#[derive(Debug, Default, Clone)]
pub struct Batch {
    /// New and updated tracks.
    pub upserts: Vec<TrackRecord>,
    /// Missing tracks whose files are back unchanged, or back and unreadable: clear the mark.
    pub returned: Vec<TrackId>,
    /// Tracks whose files their folder no longer lists: mark them missing.
    pub missing: Vec<TrackId>,
    /// Folders their parent no longer lists: mark every track under them missing.
    pub vanished: Vec<PathBuf>,
    /// Tracks whose files changed but could not be read: forget each one's mtime, so no file
    /// matches it and every scan reads the file until one reads it, and reset its analysis,
    /// which measured other audio.
    pub changed_unread: Vec<TrackId>,
    pub problems: Vec<Problem>,
    /// Paths that scanned or listed successfully, or no longer exist: clear any problems
    /// recorded for them. Applied before `problems`, so a path in both keeps the problem this
    /// batch records.
    pub cleared: Vec<PathBuf>,
}

impl Batch {
    /// How many tracks, folders and problems the batch writes, which is what sizes it.
    pub fn len(&self) -> usize {
        self.upserts.len()
            + self.returned.len()
            + self.missing.len()
            + self.vanished.len()
            + self.problems.len()
    }

    /// Whether applying the batch would change nothing. Paths to clear and tracks changed but
    /// unread count, though not toward [`Batch::len`], which sizes a batch by the tracks, folders
    /// and problems it writes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0 && self.cleared.is_empty() && self.changed_unread.is_empty()
    }
}

/// A persistence failure. A scan that hits one suspends at its last persisted cursor, and a
/// background job backs off as `jobs` describes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("store: {0}")]
pub struct StoreError(pub String);

/// A track the analysis queue handed out, with its file's size and mtime when it was picked,
/// which [`AnalysisStore::store_analysis`] checks before storing its result.
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
    /// or the file is not a regular one (`confine`), and an error when the file is gone,
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

/// Everything a scan and the queue need from persistence. Implementations are synchronous; the
/// scanner calls them from its own blocking threads. Every method of this trait and the narrower
/// ones below can fail with a [`StoreError`], which the caller handles, so a failure is never
/// mistaken for an answer.
pub trait Store: Send + Sync {
    // --- scans ---
    /// Persist a new scan of `scopes` in `library` under `trigger`, queued, and return it with
    /// the id the store assigned.
    fn create_scan(
        &self,
        library: LibraryId,
        trigger: Trigger,
        scopes: &[Scope],
    ) -> Result<Scan, StoreError>;
    /// Save `scan`'s state, cursor, progress and times over its row.
    fn update_scan(&self, scan: &Scan) -> Result<(), StoreError>;
    /// Scans left in `Running` or `Queued` by a previous process, for resume. A row that cannot
    /// be read is left out, so it does not stop the others resuming, and marked suspended, since
    /// it can never resume.
    fn interrupted_scans(&self, library: LibraryId) -> Result<Vec<Scan>, StoreError>;

    // --- files ---
    /// What the index holds directly in the folder `dir`, read in one pass however much lies
    /// below it; nothing when no root of `library` holds `dir`.
    fn indexed_in(&self, library: LibraryId, dir: &Path) -> Result<IndexedFolder, StoreError>;
    /// Apply one batch atomically and append its net effect to the change feed, and return how
    /// many tracks it marked missing that were not already. An upserted track keeps its analysis
    /// only while its file's size and mtime are unchanged. An error means nothing in the batch
    /// was persisted.
    fn apply(&self, library: LibraryId, batch: Batch) -> Result<i64, StoreError>;
    /// The paths with a recorded problem that `scope` reaches: under it, or for a Directory
    /// scope directly in it ([`Scope::contains_file`]).
    fn problems_under(&self, library: LibraryId, scope: &Scope)
    -> Result<Vec<PathBuf>, StoreError>;
    /// Whether a track not yet missing, or a problem, lies under `folder`, at any depth.
    fn indexes_under(&self, library: LibraryId, folder: &Path) -> Result<bool, StoreError>;
}

/// Where a background job records what it made of a file it could not decode, and clears the
/// problem saying a file could not be opened once it opens (`jobs`). Each call stands alone,
/// outside any scan's batch.
pub trait ProblemLog: Send + Sync {
    /// Records `problem`, replacing any problem recorded for its path.
    fn record_problem(&self, library: LibraryId, problem: Problem) -> Result<(), StoreError>;
    /// Clears a problem saying the file at `path` could not be opened
    /// ([`crate::problems::UNOPENED`]), and leaves any other.
    fn clear_unopened(&self, library: LibraryId, path: &Path) -> Result<(), StoreError>;
}

/// The analysis queue and its results (`design/scanning.md` §12).
pub trait AnalysisStore: ProblemLog {
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
}

/// The images still without a placeholder, and their placeholders (`crate::Placeholders`).
pub trait PlaceholderStore: ProblemLog {
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
