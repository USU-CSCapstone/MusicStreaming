//! Playback messages for the player's and realtime tests.

use serde_json::{Value, json};

/// Player command ID `n`, as the UUID a client would send.
pub fn command_id(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// A progress report from `device` that item `item-1`, track 1 of library 1, is `status` at
/// 0 ms, with the fields of `changes` added or replacing those. Panics if `changes` is not an
/// object.
pub fn progress_report(device: &str, status: &str, changes: Value) -> Value {
    let mut report = json!({
        "deviceId": device, "itemId": "item-1", "libraryId": "1", "trackId": "1",
        "status": status, "positionMs": 0,
    });
    report.as_object_mut().unwrap().extend(changes.as_object().unwrap().clone());
    report
}
