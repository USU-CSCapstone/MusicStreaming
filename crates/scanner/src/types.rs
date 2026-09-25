//! Domain types shared across the scanner and the store.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jewelcase_core::{AudioProperties, TagSet};
use serde::{Deserialize, Serialize};

pub type LibraryId = String;
pub type TrackId = u64;
pub type ScanId = u64;

/// What the scanner needs to know about a library (`requirements/libraries.md` §1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryConfig {
    pub id: LibraryId,
    pub roots: Vec<PathBuf>,
    /// Glob patterns matched against paths relative to their root. A matching
    /// directory is not descended; a matching file is not indexed.
    pub excludes: Vec<String>,
}

/// Why a scan started. Mirrors the API's `Scan.trigger`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Trigger {
    Initial,
    Watch,
    Scheduled,
    Manual,
    Reconfigure,
    Restore,
}

/// Mirrors the API's `Scan.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanState {
    Queued,
    Running,
    Completed,
    Cancelled,
    /// A root did not answer; the index under it was left untouched
    /// (`requirements/scanning.md` §7).
    Suspended,
}

/// How deep a scope reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Depth {
    /// Only the files directly in the directory. What the watcher enqueues
    /// for an ordinary file change.
    Directory,
    /// The directory and everything below it.
    Subtree,
}

/// A unit of scanning work: a root, or a folder inside one
/// (`design/scanning.md` §9).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    pub root: PathBuf,
    /// Absolute path, equal to or under `root`.
    pub path: PathBuf,
    pub depth: Depth,
}

impl Scope {
    pub fn root(root: impl Into<PathBuf>) -> Scope {
        let root = root.into();
        Scope {
            path: root.clone(),
            root,
            depth: Depth::Subtree,
        }
    }

    pub fn folder(root: impl Into<PathBuf>, path: impl Into<PathBuf>, depth: Depth) -> Scope {
        Scope {
            root: root.into(),
            path: path.into(),
            depth,
        }
    }

    /// Whether every file this scope would visit is also visited by `self`.
    pub fn covers(&self, other: &Scope) -> bool {
        if self.root != other.root {
            return false;
        }
        match self.depth {
            Depth::Subtree => other.path.starts_with(&self.path),
            Depth::Directory => other.depth == Depth::Directory && other.path == self.path,
        }
    }

    /// Whether `path` is a file this scope visits.
    pub fn contains_file(&self, path: &Path) -> bool {
        match self.depth {
            Depth::Subtree => path.starts_with(&self.path),
            Depth::Directory => path.parent() == Some(self.path.as_path()),
        }
    }
}

/// Mirrors the API's `Scan.progress`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub files_seen: u64,
    pub files_processed: u64,
    pub added: u64,
    pub updated: u64,
    pub moved: u64,
    pub missing: u64,
    pub problems: u64,
    pub current_path: Option<PathBuf>,
}

/// A scan: one or more scopes run under one trigger, with persisted progress
/// and a resume cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scan {
    pub id: ScanId,
    pub library: LibraryId,
    pub trigger: Trigger,
    pub scopes: Vec<Scope>,
    pub state: ScanState,
    pub started_at: Option<SystemTime>,
    pub finished_at: Option<SystemTime>,
    pub progress: ScanProgress,
    /// Index of the scope being worked, and the last directory fully
    /// persisted within it. Resume continues after this directory.
    pub cursor: Option<Cursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub scope_index: usize,
    pub after_directory: Option<PathBuf>,
}

impl Scan {
    /// Whether `other`'s work is entirely within this scan's.
    pub fn covers(&self, other: &[Scope]) -> bool {
        other
            .iter()
            .all(|o| self.scopes.iter().any(|s| s.covers(o)))
    }
}

/// Where a track's artwork was found (`requirements/scanning.md` §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtworkSource {
    Embedded,
    Sidecar(PathBuf),
}

/// Sort keys computed by the core at scan time (`design/general.md` §3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortKeys {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
}

/// Everything the scanner knows about one file, ready to persist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackRecord {
    /// `Some` when updating an existing track, `None` for a new one.
    pub id: Option<TrackId>,
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: u64,
    pub tags: TagSet,
    pub properties: AudioProperties,
    pub artwork: Option<ArtworkSource>,
    pub artist_image: Option<PathBuf>,
    pub artist_biography: Option<PathBuf>,
    /// Sidecar lyrics file, when there are no embedded lyrics.
    pub lyrics_sidecar: Option<PathBuf>,
    pub sort: SortKeys,
    pub last_seen_scan: ScanId,
}

impl TrackRecord {
    /// The display title: the tag, or the filename stem
    /// (`requirements/scanning.md` §2).
    pub fn display_title(&self) -> String {
        self.tags
            .title
            .clone()
            .unwrap_or_else(|| filename_title(&self.path))
    }
}

pub fn filename_title(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_owned())
}

/// Scan problem kinds, mirroring the API's `ScanProblemGroup.kind` plus the
/// watchdog kill the design adds (`design/scanning.md` §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProblemKind {
    Unreadable,
    PermissionDenied,
    MalformedTags,
    UnsupportedEncoding,
    CorruptAudio,
    Stalled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub path: PathBuf,
    pub kind: ProblemKind,
    pub detail: String,
    pub seen_at: SystemTime,
}

/// Per-file result of the pipeline (`design/scanning.md` §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOutcome {
    Unchanged {
        track_id: TrackId,
    },
    /// A missing track whose file is back, byte-identical.
    Returned {
        track_id: TrackId,
    },
    Added(Box<TrackRecord>),
    Updated(Box<TrackRecord>),
    Problem(Problem),
}

/// Result of analyzing one track (`design/scanning.md` §12).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisResult {
    pub analyzer_version: u32,
    /// Integrated loudness, LUFS. `None` for silence.
    pub integrated_lufs: Option<f64>,
    /// Loudness range, LU.
    pub loudness_range_lu: Option<f64>,
    /// True peak across channels, dBTP.
    pub true_peak_dbtp: Option<f64>,
    pub waveform: Waveform,
    /// Opaque sonic descriptor from the feature extractor, if one is installed.
    pub features: Option<Vec<u8>>,
}

/// Peak and RMS per bin, quantized to 0..=255, over a mono downmix. The
/// honest measurement; legibility shaping happens at render time
/// (`requirements/playback.md` §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waveform {
    pub peaks: Vec<u8>,
    pub rms: Vec<u8>,
}

/// One track's tag-derived facts, for duplicate detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateCandidate {
    pub track_id: TrackId,
    pub path: PathBuf,
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_artists: Vec<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicateReason {
    MatchingTags,
    MatchingContent,
    AlbumUnderTwoPaths,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub reason: DuplicateReason,
    pub tracks: Vec<(TrackId, PathBuf)>,
}

/// Milliseconds since the Unix epoch, saturating at zero for older clocks.
pub fn system_time_ms(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
