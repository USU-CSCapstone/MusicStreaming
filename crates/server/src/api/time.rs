//! Timestamps as the API sends and receives them. The database keeps milliseconds since the
//! Unix epoch (`design/database.md` §1), which responses read as they are and format here, and
//! which requests' RFC 3339 times are read into.

use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcDateTime};

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

/// A date and time a request gives in RFC 3339, as milliseconds after the Unix epoch. It is
/// written back as [`rfc3339`], in UTC.
pub struct Timestamp(pub i64);

impl<'de> Deserialize<'de> for Timestamp {
    /// From a JSON string [`parse_rfc3339`] reads. Anything else is refused.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Timestamp, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse_rfc3339(&text)
            .map(Timestamp)
            .ok_or_else(|| D::Error::custom("not an RFC 3339 date and time"))
    }
}

impl Serialize for Timestamp {
    /// As [`rfc3339`].
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_rfc3339(&self.0, serializer)
    }
}

/// Reads an RFC 3339 `date-time` (§5.6), such as `1996-12-19T16:39:57-08:00`, as milliseconds
/// after the Unix epoch, dropping digits past the millisecond. A leap second (`:60`) reads as the
/// last millisecond of its minute. Anything else is `None`.
fn parse_rfc3339(text: &str) -> Option<i64> {
    let at = OffsetDateTime::parse(text, &Rfc3339).ok()?;
    Some(at.unix_timestamp() * 1000 + i64::from(at.millisecond()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one format every timestamp is sent in, and that it reads back.
    #[test]
    fn formats_utc_with_milliseconds_and_reads_it_back() {
        const DAY: i64 = 86_400_000;
        let cases = [
            (0, "1970-01-01T00:00:00.000Z"),
            (-1, "1969-12-31T23:59:59.999Z"),
            (59 * DAY + 45_296_789, "1970-03-01T12:34:56.789Z"),
            (11_017 * DAY + 1, "2000-03-01T00:00:00.001Z"),
            (2_932_896 * DAY + DAY - 1, "9999-12-31T23:59:59.999Z"),
        ];
        for (ms, expected) in cases {
            assert_eq!(rfc3339(ms), expected, "{ms}");
            assert_eq!(parse_rfc3339(expected), Some(ms), "{expected} reads back");
        }
        assert_eq!(rfc3339(i64::MAX), "9999-12-31T23:59:59.999Z", "the last time there is");
    }

    /// The examples of RFC 3339 §5.8, and the ends of its four-digit years.
    #[test]
    fn reads_the_rfc_examples_with_their_offsets() {
        let cases = [
            ("1985-04-12T23:20:50.52Z", 482_196_050_520),
            ("1996-12-19T16:39:57-08:00", 851_042_397_000),
            ("1990-12-31T23:59:60Z", 662_687_999_999),
            ("1990-12-31T15:59:60-08:00", 662_687_999_999),
            ("1937-01-01T12:00:27.87+00:20", -1_041_337_172_130),
            ("1969-12-31T23:59:59.9999Z", -1),
            ("2026-10-02t14:00:00.500999+02:00", 1_790_942_400_500),
            ("2026-10-02T12:00:00.5z", 1_790_942_400_500),
            ("0000-01-01T00:00:00Z", -62_167_219_200_000),
            ("9999-12-31T23:59:59.999Z", 253_402_300_799_999),
        ];
        for (text, ms) in cases {
            assert_eq!(parse_rfc3339(text), Some(ms), "{text}");
        }
    }

    #[test]
    fn refuses_anything_but_a_full_date_time_with_an_offset() {
        let refused = [
            "now",
            "2460000",
            "2026-10-02",
            "2026-10-02T12:00:00",
            "2026-10-02T12:00Z",
            "2026-10-02T12:00:00.Z",
            "2026-10-02T12:00:00.5xZ",
            "2026-10-02T12:00:00+0200",
            "2026-10-02T12:00:00+24:00",
            "2026-10-02T12:00:00-00:60",
            "2026-10-02T24:00:00Z",
            "2026-10-02T12:00:60Z",
            "2026-1-02T12:00:00Z",
            "+2026-10-02T12:00:00Z",
            "2026-02-29T00:00:00Z",
            "２０２６-10-02T12:00:00Z",
        ];
        for text in refused {
            assert_eq!(parse_rfc3339(text), None, "{text}");
        }
    }
}
