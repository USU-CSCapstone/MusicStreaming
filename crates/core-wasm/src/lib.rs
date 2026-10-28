//! WebAssembly bindings for the shared core's queue, for the web client (`design/general.md` §3).
//!
//! Structured values cross the boundary as JSON strings, in the same shapes as the wire, so the
//! core's `serde` derives are the one description of them. Failures become JavaScript errors
//! carrying the core's message.

use jewelcase_core::{Edit, Source};
use wasm_bindgen::prelude::*;

/// A queue held in WebAssembly memory. Call `free()` when done with one, or let it be collected.
#[wasm_bindgen]
pub struct Queue(jewelcase_core::Queue);

#[wasm_bindgen]
impl Queue {
    /// Start a queue from a source, as JSON `{key, libraryId, tracks}`, playing the item at
    /// `start`. Fails on bad JSON, a reserved key or a `start` past the end.
    pub fn start(source: &str, start: usize) -> Result<Queue, JsError> {
        let source: Source = serde_json::from_str(source)?;
        Ok(Self(jewelcase_core::Queue::start(source, start)?))
    }

    /// What a start does: like `start`, then carries the Manual Queue and repeat mode of
    /// `previous`, the queue being replaced, if given, and shuffles with `seed` if given.
    pub fn launch(
        source: &str,
        start: usize,
        seed: Option<u32>,
        previous: Option<Queue>,
    ) -> Result<Queue, JsError> {
        let source: Source = serde_json::from_str(source)?;
        let previous = previous.as_ref().map(|queue| &queue.0);
        Ok(Self(jewelcase_core::Queue::launch(source, start, seed, previous)?))
    }

    /// Rebuild a queue from the JSON that [`Queue::snapshot`] or the server produced.
    #[wasm_bindgen(js_name = fromSnapshot)]
    pub fn from_snapshot(json: &str) -> Result<Queue, JsError> {
        Ok(Self(serde_json::from_str(json)?))
    }

    /// The whole queue as JSON, for storage or to restore with `fromSnapshot`.
    pub fn snapshot(&self) -> Result<String, JsError> {
        Ok(serde_json::to_string(&self.0)?)
    }

    /// Apply one edit, as JSON like `{"type":"shuffle","on":true,"seed":42}`. A failed edit
    /// throws and leaves the queue unchanged.
    pub fn apply(&mut self, edit: &str) -> Result<(), JsError> {
        let edit: Edit = serde_json::from_str(edit)?;
        Ok(self.0.apply(&edit)?)
    }

    /// The ID of the item playing now, if any.
    pub fn current(&self) -> Option<String> {
        self.0.current()
    }

    /// The Manual Queue's item IDs, in play order.
    pub fn manual(&self) -> Vec<String> {
        self.0.manual().collect()
    }

    /// Up to `limit` of the context's Up Next item IDs from `offset`, in play order. They play
    /// after the Manual Queue.
    pub fn upcoming(&self, offset: usize, limit: usize) -> Vec<String> {
        self.0.upcoming_window(offset, limit).collect()
    }

    /// Up to `limit` History item IDs from `offset`, oldest first.
    pub fn history(&self, offset: usize, limit: usize) -> Vec<String> {
        self.0.history_window(offset, limit).collect()
    }

    /// The `[libraryId, trackId]` an item plays, or nothing if the queue has no such item.
    pub fn track(&self, id: &str) -> Option<Vec<String>> {
        let track = self.0.track(id)?;
        Some(vec![track.library_id.to_owned(), track.track_id.to_owned()])
    }
}
