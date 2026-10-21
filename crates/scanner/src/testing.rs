//! Fixtures and the store conformance suite (`design/scanning.md` §15).
//!
//! Every [`Store`](crate::Store) implementation runs the same scenarios through the real
//! pipeline. The in-memory store and the server's SQLite store both run them
//! through [`store_suite!`](crate::store_suite) and must pass unchanged. The move scenarios are
//! ignored until track identity across moves is decided (`design/general.md` §11).

mod fixtures;
#[cfg(test)]
mod hooked;
mod indexing;
mod inspect;
mod jobs;
mod missing;
mod moves;
mod parked;
mod placeholders;
mod problems;
mod queue;
mod reanalysis;
mod scopes;
mod shutdown;
mod suite;
mod unread;
pub use fixtures::*;
#[cfg(test)]
pub(crate) use hooked::Hooked;
pub use indexing::*;
pub use inspect::{InspectStore, InspectTrack};
pub use jobs::*;
pub use missing::*;
pub use moves::*;
pub use parked::Parked;
pub use placeholders::*;
pub use problems::*;
pub use queue::*;
pub use reanalysis::*;
pub use scopes::*;
pub use shutdown::*;
pub use unread::*;
