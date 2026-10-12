//! Timestamps the API sends that are not read from the database, which formats its own with
//! `sql::timestamp!`.

/// `ms` after the Unix epoch as RFC 3339 UTC with milliseconds, as `sql::timestamp!` writes it.
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
    use super::super::sql::timestamp;
    use super::*;

    #[test]
    fn agrees_with_the_database() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut stmt = conn.prepare(concat!("SELECT ", timestamp!("?1"))).unwrap();
        // The epoch, leap days, century years, and the far future, each with milliseconds.
        let days = [0, 59, 60, 10_956, 11_016, 11_017, 47_541, 47_542, 2_932_896];
        for day in days {
            for ms in
                [day * 86_400_000, day * 86_400_000 + 86_399_999, day * 86_400_000 + 45_296_789]
            {
                let expected: String = stmt.query_row([ms], |row| row.get(0)).unwrap();
                assert_eq!(rfc3339(ms), expected, "{ms}");
            }
        }
    }
}
