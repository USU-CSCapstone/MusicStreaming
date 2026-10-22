//! Ids for new rows (`design/database.md` §1). SQLite ignores `DEFAULT` on an
//! `INTEGER PRIMARY KEY`, so each insert binds one drawn here.

use rusqlite::Error::ToSqlConversionFailure;

/// A random id for a new row: a positive 63-bit integer from the system's random source. Fails
/// only when that source cannot be read.
pub fn new_id() -> rusqlite::Result<i64> {
    loop {
        let bits = getrandom::u64().map_err(|error| ToSqlConversionFailure(Box::new(error)))?;
        if let Some(id) = positive(bits) {
            return Ok(id);
        }
    }
}

/// `bits` shifted right one place, which clears the sign bit, as an id, or `None` when that
/// leaves zero.
fn positive(bits: u64) -> Option<i64> {
    Some((bits >> 1).cast_signed()).filter(|&id| id > 0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{new_id, positive};

    #[test]
    fn an_id_is_any_positive_63_bit_integer() {
        assert_eq!(positive(0), None);
        assert_eq!(positive(1), None);
        assert_eq!(positive(2), Some(1));
        assert_eq!(positive(1 << 63), Some(1 << 62));
        assert_eq!(positive(u64::MAX), Some(i64::MAX));
    }

    /// Drawn ids are positive and distinct, and use the top of the range: the chance that a
    /// thousand all fall in its lower half is 2⁻¹⁰⁰⁰.
    #[test]
    fn new_ids_are_positive_distinct_and_span_63_bits() {
        let ids: HashSet<i64> = (0..1000).map(|_| new_id().unwrap()).collect();
        assert_eq!(ids.len(), 1000);
        assert!(ids.iter().all(|&id| id > 0));
        assert!(ids.iter().any(|&id| id >= 1 << 62));
    }
}
