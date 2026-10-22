//! The Jewelcase scanner (`design/scanning.md`).
//!
//! Music on disk becomes rows in the index here, and only here
//! (`requirements/general.md` §3.2). The crate reads and never writes to a
//! library; it persists through the [`Store`] trait the server implements.
//!
//! Layout follows the design: `discover` walks, `tags` reads, `sidecar`
//! resolves, `identity` classifies, `scan` runs the pipeline over a scope,
//! `queue` coalesces triggers into scans, `triggers` feeds the queue from
//! the filesystem watcher and the schedule, and `analysis` is the one decode pass.
//! `jobs` is what analysis and placeholders make of a file they could not decode, `pool` runs
//! them for every library on one set of threads, and `worker` runs each background thread until
//! its owner drops it.
//! `confine` refuses paths through symlinks, which are never followed.
//! [`TagSet`] and [`AudioProperties`] are what `tags` reads from every file,
//! and `lrc` turns synced lyrics into timed lines.

#[cfg(panic = "abort")]
compile_error!("the scanner turns parser panics into problems and needs unwinding");

mod analysis;
mod audio;
mod clock;
mod confine;
mod discover;
mod identity;
mod image;
mod jobs;
mod library;
mod lrc;
mod multi_value;
mod placeholders;
mod pool;
mod problems;
mod queue;
mod scan;
mod scope;
mod sidecar;
mod store;
mod tag_set;
mod tags;
#[cfg(feature = "testing")]
pub mod testing;
mod track;
mod triggers;
mod worker;

pub use analysis::waveform::Waveform;
pub use analysis::{ANALYZER_VERSION, AnalysisResult, Analyzer};
pub use audio::AudioProperties;
pub use confine::without_links;
pub use image::{ImageInfo, describe as describe_image};
pub use jobs::RunReport;
pub use library::{LibraryConfig, LibraryId};
pub use lrc::{SyncedLine, parse as parse_lrc};
pub use placeholders::Placeholders;
pub use pool::{Job, Pool};
pub use problems::{Problem, ProblemKind, UNOPENED, group_key as problem_group_key};
pub use queue::{Scanner, ScannerThread};
pub use scan::{
    Cursor, Scan, ScanContext, ScanId, ScanOptions, ScanProgress, ScanState, Trigger, run_scan,
};
pub use scope::{Depth, Scope};
pub use store::{
    AnalysisStore, Batch, FileToDecode, IndexedFile, IndexedLyrics, PickedTrack, PlaceholderStore,
    ProblemLog, Store, StoreError,
};
pub use tag_set::{Lyrics, PartialDate, TagSet};
pub use tags::{ReadError, read as read_tags};
pub use track::{ArtworkSource, SortKeys, TrackId, TrackRecord};
pub use triggers::{FsWatcher, scan_every};
pub use worker::{Stop, Worker};
