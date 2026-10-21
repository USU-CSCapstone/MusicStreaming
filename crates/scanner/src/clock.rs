//! Wall-clock times as milliseconds since the Unix epoch.

use std::time::SystemTime;

/// Milliseconds since the Unix epoch, saturating at zero for older clocks.
pub fn system_time_ms(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The current wall-clock time in milliseconds since the Unix epoch, as scans and problems
/// record it.
pub fn now_ms() -> i64 {
    let since_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH);
    since_epoch.map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}
