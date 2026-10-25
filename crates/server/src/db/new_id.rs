//! Ids for new rows (`design/database.md` §1). SQLite ignores `DEFAULT` on an
//! `INTEGER PRIMARY KEY`, so each insert binds one drawn here.

use rusqlite::Error::ToSqlConversionFailure;

/// A random id for a new row: a positive 63-bit integer from the system's random source. Fails
/// only when that source cannot be read.
pub fn new_id() -> rusqlite::Result<i64> {
    let bits = getrandom::u64().map_err(|error| ToSqlConversionFailure(Box::new(error)))?;
    // Shifting clears the sign bit. Zero, one draw in 2⁶³, is taken as one.
    Ok((bits >> 1).cast_signed().max(1))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::new_id;

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
