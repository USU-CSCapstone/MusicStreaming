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
    assert!(is_due(None, 0));
    let due = 1_000 + 30 * 24 * 60 * 60;
    assert!(!is_due(Some(1_000), due - 1));
    assert!(is_due(Some(1_000), due));
    assert!(!is_due(Some(u64::MAX), u64::MAX - 1), "no overflow");
}

#[test]
fn the_cursor_the_next_tracks_and_misses_are_kept_apart() {
    assert_ne!(CURSOR, miss_key(1));
    assert_ne!(NEXT, miss_key(1));
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
