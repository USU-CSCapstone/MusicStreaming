//! The queue model (`design/queue.md`): frozen lists, and spans over views of them.
//!
//! Pure: no I/O, clock or randomness.

mod cleanup;
mod edit;
mod history;
mod holding;
mod permutation;
mod previous;
mod read;
mod rewind;
mod shuffle;
mod spans;
mod state;
#[cfg(test)]
mod tests;

pub use edit::{Anchor, Edit, PlayRef};
pub use history::{HISTORY_RUNS, PlayRow};
pub use holding::{Reading, positions_holding};
pub use read::Row;
pub use spans::{Recipe, Span, SpanList, View};
pub use state::{ItemId, QueueCopy, QueueError, QueueState, Repeat, Run, Shuffle, Source};
