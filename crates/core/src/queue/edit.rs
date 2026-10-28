//! The edits a device or the server applies to a queue, and why one can fail.
//!
//! Edits name items, never positions, so they mean the same thing whatever order they arrive in
//! (`design/general.md` §3). They serialize as tagged JSON for the wire and for storage.

use super::Source;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Where an add or move lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Anchor {
    /// The front of the Manual Queue: Queue Next.
    ManualStart,
    /// The end of the Manual Queue: Queue Last.
    ManualEnd,
    /// The end of the context's Up Next.
    ContextEnd,
    /// Just ahead of an item in Up Next, in the Manual Queue or the context.
    Before { item: String },
}

/// One change to a queue. Item IDs are `<sourceKey>.<index>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Edit {
    /// Register a new source and put all its items at the anchor, in order.
    Add { source: Source, before: Anchor },
    /// Drop items from Up Next or History. Items that are gone, or playing now, are ignored.
    Remove { items: Vec<String> },
    /// Move items from Up Next or History to the anchor, keeping their relative order. Items that
    /// are gone, or playing now, are ignored.
    Move { items: Vec<String>, before: Anchor },
    /// Play an item now. From Up Next, everything ahead of it moves to History; from History, Up
    /// Next is untouched.
    Jump { item: String },
    /// Play the next item.
    Next,
    /// Play the most recent History item.
    Previous,
}

/// Why an edit or a queue could not be made. The queue is unchanged after an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The anchor names an item that is not in Up Next, or one the same edit is moving.
    AnchorGone,
    /// A source with this key is already in the queue.
    DuplicateSource,
    /// The queue's first item is past the end of its source.
    StartOutOfRange,
    /// The queue would hold more items than a 32-bit index can name.
    TooManyItems,
}

impl fmt::Display for Error {
    /// Describe the error in a short sentence.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AnchorGone => "the item to insert before is no longer in Up Next",
            Self::DuplicateSource => "a source with this key is already in the queue",
            Self::StartOutOfRange => "the starting item is past the end of its source",
            Self::TooManyItems => "the queue would hold too many items",
        })
    }
}

impl std::error::Error for Error {}
