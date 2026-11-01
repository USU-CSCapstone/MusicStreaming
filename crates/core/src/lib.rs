//! Jewelcase shared core.
//!
//! Every rule that must behave identically on the server and on a device lives
//! here (`design/general.md` §3). The crate is pure: no I/O, no clock, no
//! platform APIs. Callers pass in what it needs.

mod fold;
mod format;
mod lock;
pub mod playlist;
mod queue;
mod search;
mod sort;
mod waveform;

pub use fold::{artists_key, name_key, tag_key};
pub use format::Format;
pub use lock::Recover;
pub use queue::{
    ItemId, QueueCopy, QueueError, QueueState, Recipe, Repeat, Row, Run, Shuffle, Source, Span,
    SpanList, View,
};
pub use search::{Document, Index, Kind, Section, words};
pub use sort::{sort_key, tag_sort_key};
pub use waveform::{shape_waveform, waveform_level};
