//! Each account's queues and lists, kept in memory and saved in the background
//! (`design/queue.md`, `design/database.md` §6).
//!
//! The server holds the active queue's state and the lists it has not yet saved. A change only
//! marks its account as changed; one task saves every changed account together every second,
//! and once more as the server stops, so the database sees one write however many people are
//! listening.

mod edit;
mod hash;
mod keep;
mod read;
mod save;
mod snapshot;
mod store;
#[cfg(test)]
mod tests;

pub use edit::{Added, Applied, EditError};
pub use hash::list_hash;
pub use keep::{KeepError, NewQueue};
pub use read::{Section, Slot};
pub use snapshot::names;
pub use store::{Position, Queues};
