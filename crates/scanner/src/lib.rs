//! The Jewelcase scanner (`design/scanning.md`).
//!
//! Music on disk becomes rows in the index here, and only here
//! (`requirements/general.md` §3.2). The crate reads and never writes to a
//! library; it persists through the [`Store`] trait the server implements.
//!
//! Layout follows the design: [`discover`] walks, [`tags`] reads, [`sidecar`]
//! resolves, [`identity`] classifies, [`scan`] runs the pipeline over a scope,
//! [`queue`] coalesces triggers into scans, [`triggers`] feeds the queue from
//! the filesystem watcher and the schedule, [`governor`] pauses everything
//! when listeners need the host, and [`analysis`] is the one decode pass.
//! [`jobs`] is what analysis and placeholders make of a file they could not decode.
//! [`confine`] refuses paths through symlinks, which are never followed.
//! [`TagSet`] and [`AudioProperties`] are what [`tags`] reads from every file,
//! and [`lrc`] turns synced lyrics into timed lines.

pub mod analysis;
mod audio;
pub mod confine;
pub mod discover;
pub mod duplicates;
pub mod governor;
pub mod identity;
pub mod image;
pub mod jobs;
pub mod lrc;
mod multi_value;
pub mod placeholders;
pub mod problems;
pub mod queue;
pub mod scan;
pub mod sidecar;
pub mod store;
mod tag_set;
pub mod tags;
#[cfg(feature = "testing")]
pub mod testing;
pub mod triggers;
pub mod types;

pub use audio::AudioProperties;
pub use governor::Governor;
pub use queue::Scanner;
pub use store::{Batch, FileToDecode, IndexedFile, IndexedLyrics, MemoryStore, Store, StoreError};
pub use tag_set::{Lyrics, PartialDate, TagSet};
pub use types::*;
