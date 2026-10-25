//! The lyrics plugin's pass through the library and the misses it remembers, kept in its state
//! from one run to the next. Every lookup is refused before it leaves, as a failure, so no test
//! here reaches the network.
//!
//! Needs the packed plugin: run `plugins/lrclib-lyrics/build.sh` first.

mod common;

use common::{grants, plugin, track};
use jewelcase_plugins::testing::Memory;
use jewelcase_plugins::{Changes, Event, Grants, Host, Outcome, Permission, RunError};

/// A library of tracks `ids`, without lyrics, and empty state.
fn library(ids: impl IntoIterator<Item = u64>) -> Memory {
    let tracks = ids.into_iter().map(|id| track(id, &format!("Track {id}"))).collect();
    Memory { tracks, ..Memory::default() }
}

/// Grants under which every lookup is refused before it leaves, as a failure.
fn offline() -> Grants {
    grants(&[Permission::LibraryRead, Permission::Network], &["example.invalid"])
}

/// Runs the plugin on `library` with `grants`, on `event`.
async fn run(library: &Memory, grants: Grants, event: Event) -> Outcome {
    Host::new().unwrap().run(plugin(), grants, library.clone(), event).await
}

/// Runs the plugin on `library` with [`offline`] grants, on `event`.
async fn run_offline(library: &Memory, event: Event) -> Outcome {
    run(library, offline(), event).await
}

/// The IDs of the tracks a run looked up, from the line it logged for each, whose title is
/// `Track {id}` (see [`library`]).
fn looked_up(outcome: &Outcome) -> Vec<u64> {
    let id = |line: &String| line.split(' ').nth(2).unwrap().parse().unwrap();
    outcome.log.iter().map(id).collect()
}

/// What the plugin keeps under `key`, as the number it stores first: the cursor's track, or when
/// a miss was made.
fn kept(library: &Memory, key: &str) -> Option<u64> {
    let state = library.state.lock().unwrap();
    state.get(key).map(|bytes| u64::from_le_bytes(bytes[..8].try_into().unwrap()))
}

/// Records a miss on track `id`, made at `at` seconds since the Unix epoch.
fn miss(library: &Memory, id: u64, at: u64) {
    library.state.lock().unwrap().insert(format!("miss/{id}"), at.to_le_bytes().to_vec());
}

/// A time long enough ago for a miss to be due again, and one so late that it never is.
const LONG_AGO: u64 = 0;
const NEVER_DUE: u64 = u64::MAX / 2;

#[tokio::test]
async fn misses_are_looked_up_again_only_once_due() {
    let mut library = library(1..=4);
    library.tracks[3].has_lyrics = true;
    miss(&library, 1, LONG_AGO);
    miss(&library, 2, NEVER_DUE);

    for event in [Event::Run, Event::Scheduled] {
        let outcome = run_offline(&library, event).await;
        assert_eq!(looked_up(&outcome), [1, 3], "all but a miss not due yet, and lyrics");
        let summary = outcome.summary();
        assert!(summary.contains("1 already had lyrics"), "{summary}");
        assert!(!summary.contains("more to do"), "{summary}");
    }
    let misses = (kept(&library, "miss/1"), kept(&library, "miss/2"), kept(&library, "miss/3"));
    assert_eq!(misses, (Some(LONG_AGO), Some(NEVER_DUE), None), "a failed lookup changes nothing");
    assert_eq!(kept(&library, "cursor"), None, "the end: the next run starts over");
}

#[tokio::test]
async fn a_run_stops_after_ten_thousand_checks_and_the_next_carries_on() {
    let library = library(1..=10_002);
    (1..=10_000).for_each(|id| miss(&library, id, NEVER_DUE));
    miss(&library, 10_001, LONG_AGO);

    let first = run_offline(&library, Event::Run).await;
    assert!(first.log.is_empty(), "{:?}", first.log);
    let summary = first.result.expect("ran");
    assert!(summary.ends_with("carries on from there next time."), "{summary}");
    assert_eq!(kept(&library, "cursor"), Some(10_000));

    // The schedule carries on the same pass.
    let second = run_offline(&library, Event::Scheduled).await;
    assert_eq!(looked_up(&second), [10_001, 10_002]);
    assert_eq!(kept(&library, "cursor"), None);

    let third = run_offline(&library, Event::Run).await;
    assert!(third.log.is_empty(), "starts over: {:?}", third.log);
    assert_eq!(kept(&library, "cursor"), Some(10_000));
}

#[tokio::test]
async fn a_change_settles_or_keeps_a_miss_and_a_removal_forgets_it() {
    let mut library = library([5, 6]);
    library.tracks[0].has_lyrics = true;
    for id in [5, 6, 9] {
        miss(&library, id, LONG_AGO);
    }
    let changes = Changes { changed: vec![5, 6], removed: vec![9] };
    let outcome = run_offline(&library, Event::TracksChanged(changes)).await;
    assert!(outcome.result.is_ok(), "{}", outcome.summary());
    assert_eq!(kept(&library, "miss/5"), None, "it has lyrics now");
    assert_eq!(kept(&library, "miss/6"), Some(LONG_AGO), "its lookup failed");
    assert_eq!(kept(&library, "miss/9"), None, "it left the library");
}

#[tokio::test]
async fn a_run_cut_short_keeps_its_place_after_each_lookup() {
    let library = Memory { fail_after: Some(500), ..library(1..=501) };
    let outcome = run_offline(&library, Event::Run).await;
    // The library's failure reaches the plugin as `failed`, in the library's words.
    let failed = RunError::Reported("the library could not be read".into());
    assert_eq!(outcome.result, Err(failed));
    assert_eq!(looked_up(&outcome).len(), 500);
    assert_eq!(kept(&library, "cursor"), Some(500));
}

#[tokio::test]
async fn a_run_looks_up_first_the_tracks_a_change_had_no_time_for() {
    let library = library(1..=3);
    let left = |ids: &[u64]| ids.iter().flat_map(|id| id.to_le_bytes()).collect::<Vec<_>>();
    library.state.lock().unwrap().insert("next".into(), left(&[3, 1]));
    let outcome = run_offline(&library, Event::Run).await;
    // Failed lookups record no miss, so the pass looks them up again in its turn.
    assert_eq!(looked_up(&outcome), [3, 1, 1, 2, 3]);
    assert!(!library.state.lock().unwrap().contains_key("next"), "each is dropped once looked up");

    library.state.lock().unwrap().insert("next".into(), left(&[3]));
    let changes = Changes { changed: vec![2], removed: vec![] };
    let outcome = run_offline(&library, Event::TracksChanged(changes)).await;
    assert_eq!(looked_up(&outcome), [3, 2], "a change looks them up first too");
}
