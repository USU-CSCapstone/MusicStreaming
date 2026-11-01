//! The queue model (`design/queue.md`): frozen lists, and spans over views of them.
//!
//! Pure: no I/O, clock or randomness.

mod cleanup;
mod edit;
mod read;
mod spans;
mod state;
#[cfg(test)]
mod tests;

pub use edit::{Anchor, Edit};
pub use read::Row;
pub use spans::{Recipe, Span, SpanList, View};
pub use state::{ItemId, QueueCopy, QueueError, QueueState, Repeat, Run, Shuffle, Source};
