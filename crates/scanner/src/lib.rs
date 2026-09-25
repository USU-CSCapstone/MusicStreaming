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

pub mod analysis;
pub mod discover;
pub mod duplicates;
pub mod governor;
pub mod identity;
pub mod problems;
pub mod queue;
pub mod scan;
pub mod sidecar;
pub mod store;
pub mod tags;
pub mod triggers;
pub mod types;

pub use governor::Governor;
pub use queue::Scanner;
pub use store::{Batch, IndexedFile, MemoryStore, Store};
pub use types::*;
