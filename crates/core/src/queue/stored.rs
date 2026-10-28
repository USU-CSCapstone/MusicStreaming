//! Saving a queue without its sources, which never change once added and so are saved once
//! (`design/queue.md`).

use std::sync::Arc;

use serde::Serialize;

use super::{Queue, Repeat, Shuffled, Source};

/// A queue's state without its sources, from [`Queue::stored`]. It serializes as the queue does
/// with no `sources`, so a `Queue` deserializes it, and [`Queue::attach_sources`] completes the
/// queue.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stored<'a> {
    manual: &'a [u32],
    order: &'a [u32],
    cursor: usize,
    current: Option<u32>,
    history: &'a [u32],
    shuffle: &'a Option<Shuffled>,
    repeat: Repeat,
    copies: u32,
}

impl Queue {
    /// This queue's state without its sources. Costs nothing until serialized.
    pub fn stored(&self) -> Stored<'_> {
        // Every field named, so one added to `Queue` and left out here does not compile.
        let Queue { sources: _, manual, order, cursor, current, history, shuffle, repeat, copies } =
            self;
        Stored {
            manual,
            order,
            cursor: *cursor,
            current: *current,
            history,
            shuffle,
            repeat: *repeat,
            copies: *copies,
        }
    }

    /// Give a queue restored from [`Queue::stored`] the sources its items number, which must be
    /// the ones it was saved with.
    pub fn attach_sources(&mut self, sources: Vec<Source>) {
        self.sources = sources.into_iter().map(Arc::new).collect();
    }
}
