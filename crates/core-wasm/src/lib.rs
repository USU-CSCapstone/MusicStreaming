//! WebAssembly bindings for the shared core's queue, for the web client (`design/general.md` §3).
//!
//! Structured values cross the boundary as JSON strings, in the same shapes as the wire, so the
//! core's `serde` derives are the one description of them. Failures become JavaScript errors
//! carrying the core's message.

use jewelcase_core::{Edit, Repeat, Source};
use serde::Deserialize;
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

    /// A copy that edits apart from this queue. Cheap however long the queue is: the copy shares
    /// the sources, which never change, and copies only the order lists.
    pub fn copy(&self) -> Queue {
        Self(self.0.clone())
    }

    /// Put the Manual Queue of `from` at the front of this queue's, as a new queue starting
    /// keeps what the user queued by hand. Fails, changing nothing, if the items do not fit.
    #[wasm_bindgen(js_name = carryManual)]
    pub fn carry_manual(&mut self, from: &Queue) -> Result<(), JsError> {
        Ok(self.0.carry_manual(&from.0)?)
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

    /// The repeat mode: `"off"`, `"all"` or `"one"`.
    pub fn repeat(&self) -> String {
        match self.0.repeat() {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
        .to_owned()
    }

    /// Whether the context is shuffled.
    pub fn shuffled(&self) -> bool {
        self.0.shuffled()
    }

    /// How many items the Manual Queue holds.
    #[wasm_bindgen(js_name = manualLen)]
    pub fn manual_len(&self) -> usize {
        self.0.manual_len()
    }

    /// Up to `limit` of the Manual Queue's item IDs from `offset`, in play order.
    #[wasm_bindgen(js_name = manualWindow)]
    pub fn manual_window(&self, offset: usize, limit: usize) -> Vec<String> {
        self.0.manual_window(offset, limit).collect()
    }

    /// Up to `limit` of the context's Up Next item IDs from `offset`, in play order. They play
    /// after the Manual Queue.
    pub fn upcoming(&self, offset: usize, limit: usize) -> Vec<String> {
        self.0.upcoming_window(offset, limit).collect()
    }

    /// How many items the context's Up Next holds.
    #[wasm_bindgen(js_name = upcomingLen)]
    pub fn upcoming_len(&self) -> usize {
        self.0.upcoming_len()
    }

    /// How many items History holds.
    #[wasm_bindgen(js_name = historyLen)]
    pub fn history_len(&self) -> usize {
        self.0.history_len()
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

/// The server's `QueueSnapshot`, as `GET /me/queue` answers it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    queue_id: String,
    version: u32,
    context: serde_json::Value,
    state: jewelcase_core::Queue,
}

/// A queue as the server's snapshot gave it, with the snapshot's other fields. The whole body is
/// parsed once, here, rather than in JavaScript and again for the queue.
#[wasm_bindgen]
pub struct Fetched {
    snapshot: Snapshot,
}

#[wasm_bindgen]
impl Fetched {
    /// Parse the body of `GET /me/queue`. Fails on JSON that is not a snapshot.
    #[wasm_bindgen(js_name = parse)]
    pub fn parse(body: &str) -> Result<Fetched, JsError> {
        Ok(Self { snapshot: serde_json::from_str(body)? })
    }

    /// The queue's ID.
    #[wasm_bindgen(getter, js_name = queueId)]
    pub fn queue_id(&self) -> String {
        self.snapshot.queue_id.clone()
    }

    /// How many edits the queue has had.
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> u32 {
        self.snapshot.version
    }

    /// What the queue started from, as JSON.
    #[wasm_bindgen(getter)]
    pub fn context(&self) -> String {
        self.snapshot.context.to_string()
    }

    /// The queue itself. Read the other fields first: this uses the `Fetched` up.
    pub fn queue(self) -> Queue {
        Queue(self.snapshot.state)
    }
}
