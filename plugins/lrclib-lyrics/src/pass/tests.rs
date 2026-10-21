use std::collections::HashMap;
use std::time::Duration;

use super::*;
use crate::progress::Why;

const WRITE: Saving = Saving { can_write: true, synced_only: false };

/// A library held in memory: each track's ID and whether it has lyrics, and the plugin's state.
#[derive(Default)]
struct Memory {
    tracks: Vec<(u64, bool)>,
    state: HashMap<String, Vec<u8>>,
}

impl Store for Memory {
    type Track = (u64, bool);

    fn id(track: &(u64, bool)) -> u64 {
        track.0
    }

    fn has_lyrics(track: &(u64, bool)) -> bool {
        track.1
    }

    fn tracks(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<(u64, bool)>, String> {
        let after = self.tracks.iter().filter(|t| after.is_none_or(|after| t.0 > after));
        Ok(after.take(limit as usize).copied().collect())
    }

    fn get_tracks(&mut self, ids: &[u64]) -> Result<Vec<(u64, bool)>, String> {
        Ok(ids.iter().filter_map(|id| self.tracks.iter().find(|t| t.0 == *id).copied()).collect())
    }

    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.state.get(key).cloned())
    }

    fn set(&mut self, key: &str, value: &[u8]) -> Result<(), String> {
        self.state.insert(key.into(), value.to_vec());
        Ok(())
    }

    fn delete(&mut self, key: &str) -> Result<(), String> {
        self.state.remove(key);
        Ok(())
    }
}

/// Tracks `ids`, without lyrics.
fn library(ids: impl IntoIterator<Item = u64>) -> Memory {
    Memory { tracks: ids.into_iter().map(|id| (id, false)).collect(), ..Memory::default() }
}

/// A deadline already past, so a run stops at its first chance.
fn past() -> Instant {
    Instant::now()
}

/// A deadline no test reaches.
fn later() -> Instant {
    Instant::now() + Duration::from_secs(3600)
}

/// The tracks kept for the pass to look up first.
fn next(store: &Memory) -> Vec<u64> {
    store.state.get(NEXT).map(|bytes| progress::decode_ids(bytes).unwrap()).unwrap_or_default()
}

/// Records a miss on track `id` that is not due for a long time.
fn missed(store: &mut Memory, id: u64) {
    let miss = Miss { retry_at: u64::MAX, why: Why::Nothing };
    store.state.insert(progress::miss_key(id), miss.encode());
}

/// Runs the pass on `store` until `deadline`, answering each lookup with a miss, and returns the
/// IDs it looked up, in order.
fn pass(store: &mut Memory, deadline: Instant) -> Vec<u64> {
    let mut looked = Vec::new();
    let look = |t: &(u64, bool)| {
        looked.push(t.0);
        Ok(Seen::Missed(Why::Nothing))
    };
    resume(store, 0, WRITE, deadline, look).unwrap();
    looked
}

#[test]
fn a_change_out_of_time_leaves_its_tracks_without_lyrics_to_the_pass() {
    let mut store = library([1, 2, 3, 4]);
    store.tracks[1].1 = true;
    missed(&mut store, 3);
    missed(&mut store, 9);
    let look = |t: &(u64, bool)| panic!("looked up {} after the deadline", t.0);
    let left = changed(&mut store, &[3, 2, 1], &[9], 0, past(), look).unwrap();
    assert_eq!(left, 2, "track 2 has lyrics");
    assert_eq!(next(&store), [3, 1], "in the order the change named them");
    assert!(!store.state.contains_key("miss/3"), "the pass reaches it even past the limit");
    assert!(!store.state.contains_key("miss/9"), "it left the library");
}

#[test]
fn the_next_run_looks_up_what_a_change_left_first() {
    let mut store = library(1..=4);
    changed(&mut store, &[3, 1], &[], 0, past(), |_| panic!("out of time")).unwrap();
    // 3 and 1 are missed as they are looked up first, so the pass then goes by them.
    assert_eq!(pass(&mut store, later()), [3, 1, 2, 4]);
    assert!(next(&store).is_empty());
    assert!(!store.state.contains_key(NEXT), "the key goes once the list is done");
}

#[test]
fn each_track_left_is_dropped_once_looked_up_and_one_unanswered_is_kept() {
    let mut store = library(1..=3);
    store.state.insert(NEXT.into(), progress::encode_ids(&[2, 7, 3, 1]));
    let mut looked = Vec::new();
    let look = |t: &(u64, bool)| {
        looked.push(t.0);
        match t.0 {
            3 => Err("the request timed out".to_owned()),
            _ => Ok(Seen::Missed(Why::Nothing)),
        }
    };
    let stopped = resume(&mut store, 0, WRITE, later(), look);
    assert_eq!(stopped.err().as_deref(), Some("the request timed out"));
    assert_eq!(looked, [2, 3]);
    assert_eq!(next(&store), [7, 3, 1], "2 is dropped; 3, unanswered, is looked up first again");

    assert_eq!(pass(&mut store, later()), [3, 1], "2 was missed in the first run");
    assert!(next(&store).is_empty(), "7 left the library: dropped with the part it was asked in");
}

#[test]
fn a_catch_up_lookup_that_ends_past_the_deadline_starts_no_other() {
    let mut store = library(1..=2);
    store.state.insert(NEXT.into(), progress::encode_ids(&[1]));
    let deadline = Instant::now() + Duration::from_millis(50);
    let mut looked = Vec::new();
    let look = |t: &(u64, bool)| {
        looked.push(t.0);
        std::thread::sleep(Duration::from_millis(100));
        Ok(Seen::Missed(Why::Nothing))
    };
    resume(&mut store, 0, WRITE, deadline, look).unwrap();
    assert_eq!(looked, [1]);
}

#[test]
fn out_of_time_the_pass_looks_up_nothing() {
    let mut store = library(1..=2);
    store.state.insert(NEXT.into(), progress::encode_ids(&[2]));
    assert_eq!(pass(&mut store, past()), Vec::<u64>::new());
    assert_eq!(next(&store), [2]);
}

#[test]
fn the_tracks_left_are_kept_once_each_and_only_so_many() {
    let mut store = library(1..=(MAX_NEXT as u64 + 10));
    changed(&mut store, &[5, 6], &[], 0, past(), |_| panic!("out of time")).unwrap();
    changed(&mut store, &[6, 7], &[], 0, past(), |_| panic!("out of time")).unwrap();
    assert_eq!(next(&store), [5, 6, 7]);
    let rest: Vec<u64> = (8..=MAX_NEXT as u64 + 10).collect();
    changed(&mut store, &rest, &[], 0, past(), |_| panic!("out of time")).unwrap();
    let kept = next(&store);
    assert_eq!(kept.len(), MAX_NEXT);
    assert_eq!(kept[..4], [5, 6, 7, 8], "the earliest are kept");
}

#[test]
fn a_track_left_by_one_change_and_named_by_the_next_is_looked_up_once() {
    let mut store = library(1..=3);
    store.state.insert(NEXT.into(), progress::encode_ids(&[2, 1]));
    let mut looked = Vec::new();
    let look = |t: &(u64, bool)| {
        looked.push(t.0);
        Ok(Seen::Settled)
    };
    assert_eq!(changed(&mut store, &[2, 3], &[], 0, later(), look).unwrap(), 0);
    assert_eq!(looked, [1, 2, 3]);
    assert!(!store.state.contains_key(NEXT));

    store.state.insert(NEXT.into(), progress::encode_ids(&[2]));
    changed(&mut store, &[2], &[], 0, past(), |_| panic!("out of time")).unwrap();
    assert_eq!(next(&store), [2], "left again, once");
}

#[test]
fn a_change_in_time_looks_up_what_one_before_it_left_then_its_own() {
    let mut store = library(1..=3);
    store.state.insert(NEXT.into(), progress::encode_ids(&[1]));
    let mut looked = Vec::new();
    let look = |t: &(u64, bool)| {
        looked.push(t.0);
        Ok(Seen::Settled)
    };
    assert_eq!(changed(&mut store, &[2, 3], &[], 0, later(), look).unwrap(), 0);
    assert_eq!(looked, [1, 2, 3]);
    assert!(store.state.is_empty());
}
