//! Stage 4: track identity (`design/scanning.md` §6).
//!
//! **Deferred.** Content-derived identity that survives moves and retags
//! (`requirements/scanning.md` §6) is open decision 3 in `design/general.md`.
//! Until it is settled this stage keys on path, which is known to be wrong
//! per the requirement. What is fixed now is the interface: everything
//! stages 1–3 learned goes in, a classification comes out, and nothing
//! outside this module reads paths to decide identity.

use std::path::{Path, PathBuf};

use jewelcase_core::{AudioProperties, TagSet};

use crate::store::{IndexedFile, Store};
use crate::types::{LibraryId, TrackId};

/// Everything known about a file before identity is decided.
#[derive(Debug, Clone)]
pub struct FileFacts<'a> {
    pub root: &'a Path,
    pub path: &'a Path,
    pub size: u64,
    pub mtime_ms: u64,
    pub tags: &'a TagSet,
    pub properties: &'a AudioProperties,
    /// What the store had for this path, if anything, from stage 2.
    pub indexed: Option<&'a IndexedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// Never seen before.
    New,
    /// Same track (by whatever identity means today), content or tags changed.
    Updated { track_id: TrackId },
    /// A track last seen elsewhere. Not produced by the path-keyed placeholder.
    Moved { track_id: TrackId, from: PathBuf },
}

/// Decide what a read file is. The `store` argument is unused by the
/// placeholder but is part of the interface a content fingerprint will need
/// to look up matches.
pub fn identify(facts: &FileFacts<'_>, _library: &LibraryId, _store: &dyn Store) -> Identity {
    match facts.indexed {
        Some(indexed) => Identity::Updated {
            track_id: indexed.track_id,
        },
        None => Identity::New,
    }
}
