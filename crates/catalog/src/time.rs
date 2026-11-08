//! Timestamps as the API sends them: the database keeps milliseconds since the Unix epoch
//! (`design/database.md` §1), which responses format here.

use serde::Serializer;
use time::UtcDateTime;

/// `ms` after the Unix epoch as RFC 3339 UTC with milliseconds, such as
/// `2000-02-29T12:34:56.789Z`. A time outside the years -9999 to 9999 is written as the nearest
/// end of that range.
pub fn rfc3339(ms: i64) -> String {
    let at = UtcDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap_or(if ms < 0 { UtcDateTime::MIN } else { UtcDateTime::MAX });
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        at.millisecond()
    )
}

/// Serializes a millisecond timestamp field as [`rfc3339`]:
/// `#[serde(serialize_with = "serialize_rfc3339")]`.
pub fn serialize_rfc3339<S: Serializer>(ms: &i64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&rfc3339(*ms))
}
