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
    assert_eq!(seen(&plain, synced_only), Seen::Missed(Why::PlainOnly));
    let preview_synced_only = Saving { synced_only: true, ..PREVIEW };
    assert_eq!(seen(&plain, preview_synced_only), Seen::Missed(Why::PlainOnly));
    assert_eq!(seen(&synced, PREVIEW), Seen::Missed(Why::Unsaved));
    assert_eq!(seen(&plain, PREVIEW), Seen::Missed(Why::Unsaved));
    assert_eq!(seen(&Found::Instrumental, WRITE), Seen::Missed(Why::Nothing));
    assert_eq!(seen(&Found::Nothing, WRITE), Seen::Missed(Why::Nothing));
}

#[test]
fn a_miss_waits_thirty_days() {
    let miss = Miss::new(1_000, Why::Nothing);
    assert_eq!(miss.retry_at, 1_000 + 30 * 24 * 60 * 60);
    assert!(!miss.due(miss.retry_at - 1, WRITE));
    assert!(miss.due(miss.retry_at, WRITE));
}

#[test]
fn a_miss_is_due_as_soon_as_its_reason_is_gone() {
    let unsaved = Miss::new(1_000, Why::Unsaved);
    assert!(!unsaved.due(1_000, PREVIEW), "still can't write");
    assert!(unsaved.due(1_000, WRITE), "adding files was granted");
    let plain = Miss::new(1_000, Why::PlainOnly);
    assert!(!plain.due(1_000, Saving { synced_only: true, ..WRITE }), "still synced only");
    assert!(plain.due(1_000, WRITE), "plain lyrics are wanted now");
    let nothing = Miss::new(1_000, Why::Nothing);
    assert!(!nothing.due(1_000, WRITE));
    assert!(!nothing.due(1_000, PREVIEW));
}

#[test]
fn tracks_never_missed_are_wanted() {
    assert!(wanted(None, 0, PREVIEW));
    assert!(!wanted(Some(Miss::new(0, Why::Nothing)), 0, WRITE));
    assert!(wanted(Some(Miss::new(0, Why::Unsaved)), 0, WRITE));
}

#[test]
fn the_cursor_and_misses_are_kept_apart() {
    assert_ne!(CURSOR, miss_key(1));
}

#[test]
fn numbers_and_misses_round_trip_and_bad_ones_are_errors() {
    assert_eq!(decode(&encode(u64::MAX)), Ok(u64::MAX));
    assert_eq!(decode(b"7").unwrap_err(), "a stored number is 1 bytes, not 8");
    for why in [Why::Nothing, Why::Unsaved, Why::PlainOnly] {
        let miss = Miss::new(7, why);
        assert_eq!(Miss::decode(&miss.encode()), Ok(miss));
    }
    let mut bad = Miss::new(7, Why::Nothing).encode();
    bad[8] = 3;
    assert_eq!(Miss::decode(&bad).unwrap_err(), "a stored miss has no reason 3");
    assert_eq!(Miss::decode(&[]).unwrap_err(), "a stored miss is empty");
    assert!(Miss::decode(&encode(7)).is_err(), "a number is not a miss");
}
