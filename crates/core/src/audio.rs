//! Technical audio properties (`requirements/tracks.md` §4).

use serde::{Deserialize, Serialize};

use crate::Format;

/// What a track's audio actually is. Every field comes from the file's
/// headers, read by the scanner; nothing here is inferred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioProperties {
    pub format: Format,
    /// Duration in milliseconds. Precise enough for gapless transitions and
    /// playlist totals (`requirements/tracks.md` §2).
    pub duration_ms: u64,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    /// Audio bitrate in kbit/s where the container reports one.
    pub bitrate_kbps: Option<u32>,
    pub file_size: u64,
}
