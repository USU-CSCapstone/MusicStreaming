//! WebAssembly bindings for the shared core's queue, for the web client's queue worker
//! (`design/general.md` §7.1).
//!
//! Structured values cross the boundary as JSON text in the wire's shapes, so the core's `serde`
//! derives are the one description of them. Counts cross as `f64` (wasm-bindgen would make a
//! `u64` a `BigInt`) and track IDs as decimal strings, since IDs pass 2⁵³. Malformed input
//! throws; an edit the queue refuses returns its code.

use std::sync::Arc;

use jewelcase_core::{ItemId, QueueState, Row};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// A queue's state and the lists attached to it, held in WebAssembly memory. Call `free()` when
/// done with one.
#[wasm_bindgen]
pub struct Queue(QueueState);

/// A list as `GET /me/queue/lists/{hash}` answers it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct List {
    tracks: Vec<String>,
}

/// A row as JavaScript reads it. `play` is History's play number.
#[derive(Serialize)]
struct RowOut<'a> {
    item: String,
    source: &'a str,
    position: u64,
    track: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    play: Option<u64>,
}

impl<'a> RowOut<'a> {
    /// The row as JavaScript reads it, with its play number if it has one.
    fn new(row: Row<'a>, play: Option<u64>) -> Self {
        Self {
            item: row.item.to_string(),
            source: row.source,
            position: row.position,
            track: row.track.map(|t| t.to_string()),
            play,
        }
    }
}

/// A count or index from JavaScript. Negative, fractional and NaN values round to a whole
/// number from zero.
#[expect(clippy::cast_sign_loss, reason = "clamped to zero or above just before")]
fn whole(n: f64) -> u64 {
    n.max(0.0) as u64
}

/// Parses a list body into track IDs, failing on an ID that isn't a decimal `u64`.
fn parse_list(body: &str) -> Result<Arc<[u64]>, JsError> {
    let list: List = serde_json::from_str(body)?;
    let tracks: Result<Vec<u64>, _> = list.tracks.iter().map(|id| id.parse()).collect();
    Ok(tracks?.into())
}

/// Serializes `rows` as a JSON array.
fn rows_json<'a>(rows: impl Iterator<Item = RowOut<'a>>) -> Result<String, JsError> {
    Ok(serde_json::to_string(&rows.collect::<Vec<_>>())?)
}

#[wasm_bindgen]
impl Queue {
    /// Load a queue from the state JSON of a snapshot, or one `toJson` wrote. No list is
    /// attached. Throws on JSON that is not a state.
    #[wasm_bindgen(js_name = fromJson)]
    pub fn from_json(state: &str) -> Result<Queue, JsError> {
        Ok(Self(serde_json::from_str(state)?))
    }

    /// A copy that edits apart from this queue and shares its attached lists, so it costs
    /// O(spans) however long the lists are.
    pub fn copy(&self) -> Queue {
        Self(self.0.clone())
    }

    /// The state as JSON, without any track IDs.
    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> Result<String, JsError> {
        Ok(serde_json::to_string(&self.0)?)
    }

    /// Apply one edit, as the JSON of a `queueEdit` payload. Returns `undefined` when it applied,
    /// or the refusal's code, such as `"anchor_gone"`, with the state unchanged. Throws on JSON
    /// that is not an edit.
    pub fn apply(&mut self, edit: &str) -> Result<Option<String>, JsError> {
        let edit = serde_json::from_str(edit)?;
        Ok(self.0.apply(&edit).err().map(|e| e.code().to_owned()))
    }

    /// Attach a list, as the JSON body of `GET /me/queue/lists/{hash}`, to every source the
    /// state says has this `hash`. Returns how many there were, `0` if none. Throws, attaching
    /// nothing, on a malformed body or one of the wrong length.
    #[wasm_bindgen(js_name = attachList)]
    pub fn attach_list(&mut self, hash: &str, body: &str) -> Result<f64, JsError> {
        let tracks = parse_list(body)?;
        Ok(self.0.attach_hash(hash, &tracks).map_err(|e| JsError::new(&e.to_string()))? as f64)
    }

    /// Attach the list of a source this device made, which has no hash yet, under the `hash` the
    /// server gave it. The body is as for `attachList`. Throws if no source has the key.
    #[wasm_bindgen(js_name = attachSource)]
    pub fn attach_source(&mut self, key: &str, hash: &str, body: &str) -> Result<(), JsError> {
        let tracks = parse_list(body)?;
        self.0.attach_list(key, hash.to_owned(), tracks).map_err(|e| JsError::new(&e.to_string()))
    }

    /// The hashes of the lists the state references, as a JSON array of strings.
    #[wasm_bindgen(js_name = listHashes)]
    pub fn list_hashes(&self) -> Result<String, JsError> {
        Ok(serde_json::to_string(&self.0.list_hashes())?)
    }

    /// How many rows Up Next holds, the Manual Queue included.
    #[wasm_bindgen(js_name = upNextLen)]
    pub fn up_next_len(&self) -> f64 {
        self.0.up_next_len() as f64
    }

    /// How many rows the Manual Queue holds, at the top of Up Next.
    #[wasm_bindgen(js_name = manualLen)]
    pub fn manual_len(&self) -> f64 {
        self.0.manual.len() as f64
    }

    /// How many plays History holds.
    #[wasm_bindgen(js_name = historyLen)]
    pub fn history_len(&self) -> f64 {
        self.0.history_len() as f64
    }

    /// Up to `limit` Up Next rows from `offset`, as a JSON array of
    /// `{item, source, position, track}`. `track` is a decimal string, or `null` while the
    /// source's list isn't attached.
    #[wasm_bindgen(js_name = upNextRows)]
    pub fn up_next_rows(&self, offset: f64, limit: f64) -> Result<String, JsError> {
        let (offset, limit) = (whole(offset), whole(limit));
        let rows = (offset..offset.saturating_add(limit))
            .map_while(|i| self.0.up_next_row(i))
            .map(|row| RowOut::new(row, None));
        rows_json(rows)
    }

    /// Up to `limit` History rows, newest first, from `offset`: as `upNextRows`, each with the
    /// play's number as `play`.
    #[wasm_bindgen(js_name = historyRows)]
    pub fn history_rows(&self, offset: f64, limit: f64) -> Result<String, JsError> {
        let (offset, limit) = (whole(offset), whole(limit));
        let rows = (offset..offset.saturating_add(limit))
            .map_while(|i| self.0.history_row(i))
            .map(|p| RowOut::new(p.row, Some(p.play)));
        rows_json(rows)
    }

    /// The row of the item playing, as `upNextRows` gives rows, or `undefined` if nothing plays.
    #[wasm_bindgen(js_name = currentRow)]
    pub fn current_row(&self) -> Result<Option<String>, JsError> {
        let row = self.0.current_row().map(|row| RowOut::new(row, None));
        Ok(row.map(|row| serde_json::to_string(&row)).transpose()?)
    }

    /// The Up Next index of `item` (`<key>.<position>`), or `undefined` if it isn't in Up Next.
    /// Throws on a malformed item.
    #[wasm_bindgen(js_name = findItem)]
    pub fn find_item(&self, item: &str) -> Result<Option<f64>, JsError> {
        let item: ItemId = item.parse().map_err(|e: String| JsError::new(&e))?;
        Ok(self.0.find_item(&item).map(|i| i as f64))
    }
}
