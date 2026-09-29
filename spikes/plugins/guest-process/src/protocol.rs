//! The out-of-process baseline's wire protocol, shared with the host by path.
//!
//! Frames are a little-endian u32 length and a JSON body, the shape most
//! out-of-process plugin protocols take (LSP, MCP). A call may be interrupted by
//! the guest asking the host for library data or state before it answers.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Track {
    pub id: u64,
    pub title: String,
    pub artist: Option<String>,
    pub duration_us: u64,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum Call {
    Noop,
    Echo { s: String },
    ScanTitles { needle: String, batch: u32 },
    Counter,
    PersistedCounter,
    Spin,
    Crash,
    Hog { mb: u32 },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ToGuest {
    Call(Call),
    Tracks { page: Vec<Track> },
    State { value: Option<Vec<u8>> },
    Ack,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ToHost {
    Done { value: serde_json::Value },
    Tracks { offset: u32, limit: u32 },
    StateGet { key: String },
    StateSet { key: String, value: Vec<u8> },
}
