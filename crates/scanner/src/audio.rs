//! Technical audio properties (`requirements/tracks.md` §4).

use std::time::Duration;

use jewelcase_core::Format;

/// What a track's audio actually is. Every field comes from the file's
/// headers, read by the scanner; nothing here is inferred.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

/// `duration` in milliseconds, or 0, as for a duration that cannot be read, when a file's
/// headers claim more than `u32::MAX` ms: a claim longer than about 49 days is not a real
/// recording, and keeping it could overflow its album's and library's totals.
pub fn duration_ms(duration: Duration) -> u64 {
    u32::try_from(duration.as_millis()).map_or(0, u64::from)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::duration_ms;

    #[test]
    fn a_duration_over_49_days_is_unknown() {
        assert_eq!(duration_ms(Duration::from_millis(180_000)), 180_000);
        let longest = u64::from(u32::MAX);
        assert_eq!(duration_ms(Duration::from_millis(longest)), longest);
        assert_eq!(duration_ms(Duration::from_millis(longest + 1)), 0);
        assert_eq!(duration_ms(Duration::MAX), 0);
    }
}
