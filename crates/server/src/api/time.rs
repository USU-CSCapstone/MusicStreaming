//! Timestamps as the API sends them. The database keeps milliseconds since the Unix epoch
//! (`design/database.md` §1), which responses read as they are and format here.

/// `ms` after the Unix epoch as RFC 3339 UTC with milliseconds, such as
/// `2000-02-29T12:34:56.789Z`.
pub fn rfc3339(ms: i64) -> String {
    let (days, ms) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Howard Hinnant's `civil_from_days`: the Gregorian date `days` after 1970-01-01, counted in
    // 400-year eras that begin on 1 March, so the leap day ends a year.
    let z = days + 719_468;
    let (era, day_of_era) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 { month_from_march + 3 } else { month_from_march - 9 };
    let year = era * 400 + year_of_era + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// Serializes a millisecond timestamp field as [`rfc3339`]:
/// `#[serde(serialize_with = "serialize_rfc3339")]`.
pub fn serialize_rfc3339<S: serde::Serializer>(ms: &i64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&rfc3339(*ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dates_across_leap_days_centuries_and_eras() {
        const DAY: i64 = 86_400_000;
        let cases = [
            (0, "1970-01-01T00:00:00.000Z"),
            (-1, "1969-12-31T23:59:59.999Z"),
            (59 * DAY + 45_296_789, "1970-03-01T12:34:56.789Z"),
            (10_956 * DAY + DAY - 1, "1999-12-31T23:59:59.999Z"),
            (11_016 * DAY, "2000-02-29T00:00:00.000Z"),
            (11_017 * DAY + 1, "2000-03-01T00:00:00.001Z"),
            (47_540 * DAY, "2100-02-28T00:00:00.000Z"),
            (47_541 * DAY, "2100-03-01T00:00:00.000Z"),
            (2_932_896 * DAY + DAY - 1, "9999-12-31T23:59:59.999Z"),
        ];
        for (ms, expected) in cases {
            assert_eq!(rfc3339(ms), expected, "{ms}");
        }
    }
}
