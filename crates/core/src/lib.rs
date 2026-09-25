//! Jewelcase shared core.
//!
//! Every rule that must behave identically on the server and on a device lives
//! here (`design/general.md` §3). The crate is pure: no I/O, no clock, no
//! platform APIs. Callers pass in what it needs.

pub mod audio;
pub mod fold;
pub mod format;
pub mod lrc;
pub mod multi_value;
pub mod sort;
pub mod tags;

pub use audio::AudioProperties;
pub use format::Format;
pub use tags::{Lyrics, PartialDate, TagSet};
