//! What the owner's other devices are told when a playlist changes: the spec's `playlist`
//! realtime message, sent to every connection of the user once the change is committed.
//!
//! A device at the previous version applies the ops as they were applied, so a small edit is a
//! small message and no item list is ever pushed (`requirements/realtime.md` §4). An add names
//! its tracks by ID, which a device resolves as it does any track it lists.

use serde::Serialize;

use super::representation::Playlist;
use crate::api::Id;
use crate::api::time::serialize_rfc3339;

/// The most the ops of one message may take as JSON. Messages are at most 64 KiB, so a bigger
/// edit, such as a playlist of thousands of tracks added at once, is announced without them.
const MAX_OPS_BYTES: usize = 32 * 1024;

/// The spec's `PlaylistAppliedOp`: an op as the server applied it. An add holds the tracks its
/// playables expanded to, less any purged meanwhile, and the time they were added.
#[derive(Serialize)]
#[serde(tag = "op", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum AppliedOp {
    Add {
        #[serde(skip_serializing_if = "Option::is_none")]
        at: Option<usize>,
        track_ids: Vec<Id>,
        #[serde(serialize_with = "serialize_rfc3339")]
        added_at: i64,
    },
    Remove {
        positions: Vec<usize>,
    },
    Move {
        positions: Vec<usize>,
        at: usize,
    },
}

/// The payload of the spec's `playlist` message.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    library_id: Id,
    playlist_id: Id,
    version: i64,
    playlist: Option<Playlist>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ops: Option<Vec<AppliedOp>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    refetch: bool,
}

impl Change {
    /// A playlist just made, or edited: `playlist` is it as the change left it. `ops` are what
    /// an edit applied to its items, empty for one that changed only its details. Too many to
    /// send are replaced by `refetch`, for the device to read the items again.
    pub fn of(playlist: Playlist, ops: Vec<AppliedOp>) -> Change {
        let small = serde_json::to_string(&ops).is_ok_and(|json| json.len() <= MAX_OPS_BYTES);
        Change {
            library_id: playlist.library_id,
            playlist_id: playlist.id,
            version: playlist.version,
            playlist: Some(playlist),
            refetch: !ops.is_empty() && !small,
            ops: (!ops.is_empty() && small).then_some(ops),
        }
    }

    /// A playlist deleted at `version`, its last.
    pub fn deleted(library: i64, id: i64, version: i64) -> Change {
        Change {
            library_id: Id(library),
            playlist_id: Id(id),
            version,
            playlist: None,
            ops: None,
            refetch: false,
        }
    }
}
