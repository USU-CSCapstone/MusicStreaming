//! Fixtures and the pipeline suite (`design/scanning.md` §15).
//!
//! The pipeline scenarios run through the real scanner against a real [`Store`](crate::Store):
//! the server's SQLite store runs every one through [`store_suite!`](crate::store_suite). Those
//! that need the store slow, failing, or holding scans up wrap it in [`Hooked`]. The move
//! scenarios are ignored until track identity across moves is decided (`design/general.md` §11).
//! [`BareStore`] serves tests that need a store but no index.

mod albums;
mod analysis;
mod bare;
mod events;
mod fixtures;
mod hooked;
mod indexing;
mod inspect;
mod jobs;
mod missing;
mod moves;
mod placeholders;
mod problems;
mod queue;
mod reanalysis;
mod scan;
mod scopes;
mod shutdown;
mod sidecars;
mod suite;
mod unread;
pub use albums::*;
pub use analysis::*;
pub use bare::BareStore;
pub use events::*;
pub use fixtures::*;
pub use hooked::Hooked;
pub use indexing::*;
pub use inspect::{InspectStore, InspectTrack};
pub use jobs::*;
/// The tag keys fixtures are written with, so tests that tag files need no `lofty` of their own.
pub use lofty::tag::ItemKey;

/// How the scanner reads a file's claimed duration, so tests can make records the way it does.
pub use crate::audio::duration_ms;
pub use missing::*;
pub use moves::*;
pub use placeholders::*;
pub use problems::*;
pub use queue::*;
pub use reanalysis::*;
pub use scan::*;
pub use scopes::*;
pub use shutdown::*;
pub use sidecars::*;
pub use unread::*;
