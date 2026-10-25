use super::*;

const WRITE: Saving = Saving { can_write: true, synced_only: false };
const PREVIEW: Saving = Saving { can_write: false, synced_only: false };

#[test]
fn only_lyrics_to_save_settle_a_track() {
    let synced = Found::Synced("[00:01]a".into());
    let plain = Found::Plain("a".into());
    let synced_only = Saving { synced_only: true, ..WRITE };
    assert_eq!(seen(&synced, WRITE), Seen::Settled);
    assert_eq!(seen(&synced, synced_only), Seen::Settled);
    assert_eq!(seen(&plain, WRITE), Seen::Settled);
    assert_eq!(seen(&plain, synced_only), Seen::Missed);
    assert_eq!(seen(&synced, PREVIEW), Seen::Missed);
    assert_eq!(seen(&plain, PREVIEW), Seen::Missed);
    assert_eq!(seen(&Found::Instrumental, WRITE), Seen::Missed);
    assert_eq!(seen(&Found::Nothing, WRITE), Seen::Missed);
}

#[test]
fn a_miss_waits_thirty_days() {
    assert!(is_due(None, 0, 0));
    let due = 1_000 + 30 * 24 * 60 * 60;
    assert!(!is_due(Some(1_000), due - 1, 0));
    assert!(is_due(Some(1_000), due, 0));
    assert!(!is_due(Some(u64::MAX), u64::MAX - 1, 0), "no overflow");
}

#[test]
fn a_miss_from_before_the_mode_changed_is_due() {
    assert!(is_due(Some(999), 1_000, 1_000));
    assert!(!is_due(Some(1_000), 1_000, 1_000), "made under the new mode");
    assert!(!is_due(Some(1_001), 1_002, 1_000));
}

#[test]
fn the_mode_changes_when_either_part_does() {
    let kept = Some((WRITE, 5));
    assert_eq!(mode_since(kept, WRITE, 9), 5);
    assert_eq!(mode_since(kept, PREVIEW, 9), 9);
    assert_eq!(mode_since(Some((WRITE, 50)), WRITE, 9), 9, "a clock that was ahead");
    assert_eq!(mode_since(kept, Saving { synced_only: true, ..WRITE }, 9), 9);
    assert_eq!(mode_since(None, WRITE, 9), 0, "no mode kept: misses stand");
}

#[test]
fn a_mode_round_trips_and_a_bad_one_is_none() {
    let both = Saving { can_write: true, synced_only: true };
    assert_eq!(decode_mode(&encode_mode(both, u64::MAX)), Some((both, u64::MAX)));
    assert_eq!(decode_mode(&encode_mode(PREVIEW, 7)), Some((PREVIEW, 7)));
    assert_eq!(decode_mode(&[0; 9]), None);
}

#[test]
fn the_cursor_the_next_tracks_and_misses_are_kept_apart() {
    assert_ne!(CURSOR, miss_key(1));
    assert_ne!(NEXT, miss_key(1));
    assert_ne!(MODE, miss_key(1));
    assert_ne!(MODE, CURSOR);
    assert_ne!(MODE, NEXT);
    assert_ne!(CURSOR, NEXT);
}

#[test]
fn track_ids_round_trip_and_a_bad_length_is_an_error() {
    assert_eq!(decode_ids(&encode_ids(&[3, u64::MAX, 1])), Ok(vec![3, u64::MAX, 1]));
    assert_eq!(decode_ids(&[]), Ok(vec![]));
    let error = "stored track IDs are 9 bytes, not a multiple of 8";
    assert_eq!(decode_ids(&[0; 9]).unwrap_err(), error);
}

#[test]
fn numbers_round_trip_and_a_bad_one_is_an_error() {
    assert_eq!(decode(&encode(u64::MAX)), Ok(u64::MAX));
    assert_eq!(decode(b"7").unwrap_err(), "a stored number is 1 bytes, not 8");
}
