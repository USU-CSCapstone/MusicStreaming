//! A scan as the store keeps it and the API reports it: its trigger, state, progress and
//! resume cursor.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::library::LibraryId;
use crate::scope::Scope;

/// A scan's id, as the store assigns it.
pub type ScanId = i64;

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

/// Mirrors the API's `Scan.progress`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanProgress {
    pub files_seen: i64,
    pub files_processed: i64,
    pub added: i64,
    pub updated: i64,
    pub moved: i64,
    pub missing: i64,
    pub problems: i64,
    pub current_path: Option<PathBuf>,
}

/// A scan: one or more scopes run under one trigger, with persisted progress
/// and a resume cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    pub id: ScanId,
    pub library: LibraryId,
    pub trigger: Trigger,
    pub scopes: Vec<Scope>,
    pub state: ScanState,
    /// Milliseconds since the Unix epoch.
    pub started_at: Option<i64>,
    /// Milliseconds since the Unix epoch.
    pub finished_at: Option<i64>,
    pub progress: ScanProgress,
    /// Index of the scope being worked, and the last directory fully
    /// persisted within it. Resume continues after this directory.
    pub cursor: Option<Cursor>,
}

/// Where an interrupted scan resumes: the index of the scope being worked in [`Scan::scopes`],
/// and the last directory fully persisted within it, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub scope_index: i64,
    pub after_directory: Option<PathBuf>,
}

impl Scan {
    /// The scan `id` of `scopes` in `library` under `trigger`, queued: not started, with no
    /// progress and no cursor. What [`Store::create_scan`](crate::Store::create_scan) returns.
    pub fn queued(id: ScanId, library: LibraryId, trigger: Trigger, scopes: Vec<Scope>) -> Scan {
        Scan {
            id,
            library,
            trigger,
            scopes,
            state: ScanState::Queued,
            started_at: None,
            finished_at: None,
            progress: ScanProgress::default(),
            cursor: None,
        }
    }

    /// Whether `other`'s work is entirely within this scan's.
    pub fn covers(&self, other: &[Scope]) -> bool {
        other.iter().all(|o| self.scopes.iter().any(|s| s.covers(o)))
    }
}
