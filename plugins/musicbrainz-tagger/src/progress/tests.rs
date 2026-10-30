use super::*;

#[test]
fn a_miss_waits_thirty_days() {
    assert!(is_due(None, 0, 0));
    let due = 1_000 + 30 * 24 * 60 * 60;
    assert!(!is_due(Some(1_000), due - 1, 0));
    assert!(is_due(Some(1_000), due, 0));
    assert!(!is_due(Some(u64::MAX), u64::MAX - 1, 0), "no overflow");
}

#[test]
fn a_miss_from_before_writing_was_granted_is_due() {
    assert!(is_due(Some(999), 1_000, 1_000));
    assert!(is_due(Some(1_000), 1_000, 1_000), "the same second may be before the change");
    assert!(!is_due(Some(1_001), 1_002, 1_000), "made since");
}

#[test]
fn the_mode_changes_when_writing_is_granted_or_taken_away() {
    assert_eq!(mode_since(Some((true, 5)), true, 9), 5);
    assert_eq!(mode_since(Some((false, 5)), true, 9), 9);
    assert_eq!(mode_since(Some((true, 5)), false, 9), 9);
    assert_eq!(mode_since(Some((true, 50)), true, 9), 9, "a clock that was ahead");
    assert_eq!(mode_since(None, true, 9), 0, "nothing kept: misses stand");
}

#[test]
fn a_mode_round_trips_and_a_bad_one_is_none() {
    assert_eq!(decode_mode(&encode_mode(true, u64::MAX)), Some((true, u64::MAX)));
    assert_eq!(decode_mode(&encode_mode(false, 7)), Some((false, 7)));
    assert_eq!(decode_mode(&[0; 8]), None);
    assert_eq!(decode_mode(&[0; 10]), None);
}

#[test]
fn the_cursor_the_mode_and_misses_are_kept_apart() {
    assert_ne!(CURSOR, miss_key(1));
    assert_ne!(MODE, miss_key(1));
    assert_ne!(MODE, CURSOR);
}

#[test]
fn numbers_round_trip_and_a_bad_one_is_an_error() {
    assert_eq!(decode(&encode(u64::MAX)), Ok(u64::MAX));
    assert_eq!(decode(b"7").unwrap_err(), "a stored number is 1 bytes, not 8");
}
