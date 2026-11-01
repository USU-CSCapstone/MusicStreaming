//! The queue model (`design/queue.md`): frozen lists, and spans over views of them.
//!
//! Pure: no I/O, clock or randomness.

mod spans;

pub use spans::{List, Recipe, Span, SpanList, View};
