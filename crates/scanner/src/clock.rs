//! Wall-clock times as milliseconds since the Unix epoch.

use std::time::SystemTime;

/// Milliseconds from the Unix epoch to `t`, saturating at zero for an earlier time and at
/// `i64::MAX` for a later one, so the store can keep any file's time.
pub fn system_time_ms(t: SystemTime) -> u64 {
    since_epoch_ms(t).unsigned_abs()
}

/// The current wall-clock time in milliseconds since the Unix epoch, as the database records
/// times.
pub fn now_ms() -> i64 {
    since_epoch_ms(SystemTime::now())
}

/// Milliseconds from the Unix epoch to `t`, saturating at zero and at `i64::MAX`.
fn since_epoch_ms(t: SystemTime) -> i64 {
    let since_epoch = t.duration_since(SystemTime::UNIX_EPOCH);
    since_epoch.map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::system_time_ms;

    #[test]
    fn a_file_time_saturates_at_both_ends() {
        let epoch = SystemTime::UNIX_EPOCH;
        assert_eq!(system_time_ms(epoch - Duration::from_secs(1)), 0);
        assert_eq!(system_time_ms(epoch + Duration::from_millis(1_500)), 1_500);
        let far = epoch + Duration::from_secs(1 << 60);
        assert_eq!(system_time_ms(far), i64::MAX.unsigned_abs());
    }
}
