//! Wall-clock times as milliseconds since the Unix epoch.

use std::time::SystemTime;

/// Milliseconds since the Unix epoch, saturating at zero for older clocks.
pub fn system_time_ms(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The current wall-clock time in milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    system_time_ms(SystemTime::now())
}
