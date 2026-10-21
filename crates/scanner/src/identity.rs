//! Stage 4: track identity (`design/scanning.md` §6).
//!
//! **Deferred.** Content-derived identity that survives moves and retags
//! (`requirements/scanning.md` §6) is open decision 3 in `design/general.md`.
//! Until it is settled this stage keys on path, which is known to be wrong
//! per the requirement: a file is the track the index holds at its path, or a
//! new one. This classifies every file that was read; a file skipped at stage 2, or a known
//! file stage 3 cannot read, keeps the track the index holds at its path without reaching it.

use crate::store::IndexedFile;
use crate::types::TrackId;

/// What a read file is to the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// Never seen before.
    New,
    /// Same track (by whatever identity means today), content or tags changed.
    Updated { track_id: TrackId },
}

/// Decide what a read file is from what the index held at its path in stage 2, if anything.
pub fn identify(indexed: Option<&IndexedFile>) -> Identity {
    match indexed {
        Some(indexed) => Identity::Updated { track_id: indexed.track_id },
        None => Identity::New,
    }
}
