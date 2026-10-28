use super::*;
use std::time::{Duration, Instant};

/// A source of `n` tracks named `<key>-0`, `<key>-1`, ... in library `lib`.
fn source(key: &str, n: usize) -> Source {
    Source {
        key: key.into(),
        library_id: "lib".into(),
        tracks: (0..n).map(|i| format!("{key}-{i}")).collect(),
    }
}

/// A queue playing album `a` of five tracks from its first.
fn album() -> Queue {
    Queue::start(source("a", 5), 0).unwrap()
}

/// Owned copies of the given item IDs.
fn strings(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).into()).collect()
}

/// An anchor just ahead of `item`.
fn before(item: &str) -> Anchor {
    Anchor::Before { item: item.into() }
}

/// Add `src` at `at`, expecting success.
fn add(q: &mut Queue, src: Source, at: Anchor) {
    q.apply(&Edit::Add { source: src, before: at }).unwrap();
}

/// Move the named items to `at`.
fn mv(q: &mut Queue, ids: &[&str], at: Anchor) -> Result<(), Error> {
    q.apply(&Edit::Move { items: strings(ids), before: at })
}

/// Remove the named items, expecting success.
fn remove(q: &mut Queue, ids: &[&str]) {
    q.apply(&Edit::Remove { items: strings(ids) }).unwrap();
}

/// Jump to `id`, expecting success.
fn jump(q: &mut Queue, id: &str) {
    q.apply(&Edit::Jump { item: id.into() }).unwrap();
}

/// The Manual Queue's item IDs.
fn manual(q: &Queue) -> Vec<String> {
    q.manual().collect()
}

/// The context's Up Next item IDs.
fn upcoming(q: &Queue) -> Vec<String> {
    q.upcoming().collect()
}

/// The History item IDs, oldest first.
fn history(q: &Queue) -> Vec<String> {
    q.history().collect()
}

#[test]
fn start_lays_out_sections_and_lookups() {
    let q = Queue::start(source("a", 4), 1).unwrap();
    assert_eq!(q.current().as_deref(), Some("a.1"));
    assert_eq!(upcoming(&q), ["a.2", "a.3"]);
    assert!(manual(&q).is_empty());
    assert!(history(&q).is_empty());
    assert_eq!(q.track("a.2"), Some(TrackRef { library_id: "lib", track_id: "a-2" }));
}

#[test]
fn start_past_the_end_fails() {
    assert_eq!(Queue::start(source("a", 2), 2), Err(Error::StartOutOfRange));
    assert_eq!(Queue::start(source("a", 0), 0), Err(Error::StartOutOfRange));
}

#[test]
fn lookups_ignore_unknown_ids() {
    let mut q = album();
    add(&mut q, source("x.y", 2), Anchor::ManualEnd);
    assert_eq!(q.track("x.y.1").map(|t| t.track_id), Some("x.y-1"));
    assert_eq!(manual(&q), ["x.y.0", "x.y.1"]);
    for id in ["a.5", "a.-1", "a", "b.0", "a.x", ""] {
        assert_eq!(q.track(id), None, "{id}");
    }
}

#[test]
fn add_queue_next_and_last() {
    let mut q = album();
    add(&mut q, source("m", 2), Anchor::ManualEnd);
    add(&mut q, source("n", 1), Anchor::ManualEnd);
    add(&mut q, source("p", 1), Anchor::ManualStart);
    assert_eq!(manual(&q), ["p.0", "m.0", "m.1", "n.0"]);
    assert_eq!(upcoming(&q), ["a.1", "a.2", "a.3", "a.4"]);
}

#[test]
fn add_before_an_item_and_at_the_end_of_the_context() {
    let mut q = album();
    add(&mut q, source("m", 1), Anchor::ManualEnd);
    add(&mut q, source("b", 2), before("a.3"));
    add(&mut q, source("c", 1), before("m.0"));
    add(&mut q, source("d", 1), Anchor::ContextEnd);
    assert_eq!(manual(&q), ["c.0", "m.0"]);
    assert_eq!(upcoming(&q), ["a.1", "a.2", "b.0", "b.1", "a.3", "a.4", "d.0"]);
}

#[test]
fn a_queue_holds_at_most_max_items() {
    assert_eq!(Queue::start(source("big", MAX_ITEMS + 1), 0), Err(Error::TooManyItems));
    let mut q = Queue::start(source("big", MAX_ITEMS - 5), 0).unwrap();
    let before = q.clone();
    let edit = Edit::Add { source: source("m", 6), before: Anchor::ManualEnd };
    assert_eq!(q.apply(&edit), Err(Error::TooManyItems));
    assert_eq!(q, before);
    add(&mut q, source("m", 5), Anchor::ManualEnd);
    assert_eq!(manual(&q).len(), 5);
}

#[test]
fn add_rejects_a_known_key_and_leaves_the_queue_alone() {
    let mut q = album();
    let edit = Edit::Add { source: source("a", 1), before: Anchor::ManualEnd };
    assert_eq!(q.apply(&edit), Err(Error::DuplicateSource));
    assert_eq!(q, album());
}

#[test]
fn remove_from_up_next_and_history() {
    let mut q = album();
    add(&mut q, source("m", 2), Anchor::ManualEnd);
    q.apply(&Edit::Next).unwrap();
    q.apply(&Edit::Next).unwrap();
    q.apply(&Edit::Next).unwrap();
    assert_eq!(history(&q), ["a.0", "m.0", "m.1"]);
    remove(&mut q, &["m.0", "a.3"]);
    assert_eq!(history(&q), ["a.0", "m.1"]);
    assert_eq!(upcoming(&q), ["a.2", "a.4"]);
}

#[test]
fn move_within_manual_and_across_to_the_context() {
    let mut q = album();
    add(&mut q, source("m", 4), Anchor::ManualEnd);
    mv(&mut q, &["m.3", "m.1"], Anchor::ManualStart).unwrap();
    assert_eq!(manual(&q), ["m.1", "m.3", "m.0", "m.2"]);
    mv(&mut q, &["m.2", "m.1"], before("a.2")).unwrap();
    assert_eq!(manual(&q), ["m.3", "m.0"]);
    assert_eq!(upcoming(&q), ["a.1", "m.1", "m.2", "a.2", "a.3", "a.4"]);
    mv(&mut q, &["a.4", "a.1"], Anchor::ManualEnd).unwrap();
    assert_eq!(manual(&q), ["m.3", "m.0", "a.1", "a.4"]);
}

#[test]
fn move_within_the_context() {
    let mut q = album();
    mv(&mut q, &["a.4"], before("a.1")).unwrap();
    mv(&mut q, &["a.1"], Anchor::ContextEnd).unwrap();
    assert_eq!(upcoming(&q), ["a.4", "a.2", "a.3", "a.1"]);
}

#[test]
fn move_from_history_back_to_up_next() {
    let mut q = album();
    for _ in 0..3 {
        q.apply(&Edit::Next).unwrap();
    }
    assert_eq!(history(&q), ["a.0", "a.1", "a.2"]);
    mv(&mut q, &["a.0", "a.2"], before("a.4")).unwrap();
    assert_eq!(history(&q), ["a.1"]);
    assert_eq!(upcoming(&q), ["a.0", "a.2", "a.4"]);
    assert_eq!(q.current().as_deref(), Some("a.3"));
    assert_eq!(q.order, [1, 3, 0, 2, 4]);
    assert_eq!(q.cursor, 2);
    mv(&mut q, &["a.1"], Anchor::ManualStart).unwrap();
    assert_eq!(manual(&q), ["a.1"]);
    assert!(history(&q).is_empty());
}

#[test]
fn move_cannot_use_its_own_item_or_a_non_upcoming_item_as_anchor() {
    let mut q = album();
    q.apply(&Edit::Next).unwrap();
    let fresh = q.clone();
    assert_eq!(mv(&mut q, &["a.2", "a.3"], before("a.3")), Err(Error::AnchorGone));
    assert_eq!(mv(&mut q, &["a.2"], before("a.0")), Err(Error::AnchorGone));
    assert_eq!(mv(&mut q, &["a.2"], before("a.1")), Err(Error::AnchorGone));
    assert_eq!(q, fresh);
}

#[test]
fn duplicates_are_distinct_items() {
    let mut q = album();
    let again = Source {
        key: "b".into(),
        library_id: "lib".into(),
        tracks: vec!["a-2".into(), "a-2".into()],
    };
    add(&mut q, again, Anchor::ManualEnd);
    assert_eq!(q.track("b.0"), q.track("b.1"));
    assert_eq!(q.track("b.0").map(|t| t.track_id), q.track("a.2").map(|t| t.track_id));
    remove(&mut q, &["b.1"]);
    assert_eq!(manual(&q), ["b.0"]);
    assert_eq!(upcoming(&q), ["a.1", "a.2", "a.3", "a.4"]);
    mv(&mut q, &["b.0"], before("a.2")).unwrap();
    assert_eq!(upcoming(&q), ["a.1", "b.0", "a.2", "a.3", "a.4"]);
}

#[test]
fn jump_in_up_next_moves_everything_ahead_to_history() {
    let mut q = album();
    add(&mut q, source("m", 2), Anchor::ManualEnd);
    jump(&mut q, "a.3");
    assert_eq!(q.current().as_deref(), Some("a.3"));
    assert_eq!(history(&q), ["a.0", "m.0", "m.1", "a.1", "a.2"]);
    assert!(manual(&q).is_empty());
    assert_eq!(upcoming(&q), ["a.4"]);
}

#[test]
fn jump_within_the_manual_queue_leaves_the_context() {
    let mut q = album();
    add(&mut q, source("m", 3), Anchor::ManualEnd);
    jump(&mut q, "m.1");
    assert_eq!(q.current().as_deref(), Some("m.1"));
    assert_eq!(history(&q), ["a.0", "m.0"]);
    assert_eq!(manual(&q), ["m.2"]);
    assert_eq!(upcoming(&q), ["a.1", "a.2", "a.3", "a.4"]);
}

#[test]
fn jump_in_history_plays_now_and_leaves_up_next_alone() {
    let mut q = album();
    add(&mut q, source("m", 1), Anchor::ManualEnd);
    for _ in 0..2 {
        q.apply(&Edit::Next).unwrap();
    }
    let (m, up) = (manual(&q), upcoming(&q));
    assert_eq!(history(&q), ["a.0", "m.0"]);
    jump(&mut q, "a.0");
    assert_eq!(q.current().as_deref(), Some("a.0"));
    assert_eq!(history(&q), ["m.0", "a.1"]);
    assert_eq!((manual(&q), upcoming(&q)), (m, up));
}

#[test]
fn jump_to_the_current_item_or_a_gone_item_does_nothing() {
    let mut q = album();
    let before = q.clone();
    jump(&mut q, "a.0");
    jump(&mut q, "zzz.9");
    assert_eq!(q, before);
}

#[test]
fn next_plays_manual_first_then_the_context_and_ends() {
    let mut q = Queue::start(source("a", 2), 0).unwrap();
    add(&mut q, source("m", 1), Anchor::ManualEnd);
    let mut played = vec![q.current().unwrap()];
    for _ in 0..3 {
        q.apply(&Edit::Next).unwrap();
        played.extend(q.current());
    }
    assert_eq!(played, ["a.0", "m.0", "a.1"]);
    assert_eq!(q.current(), None);
    assert_eq!(history(&q), ["a.0", "m.0", "a.1"]);
    q.apply(&Edit::Next).unwrap();
    assert_eq!(history(&q).len(), 3);
}

#[test]
fn previous_replays_history_and_puts_the_current_item_back() {
    let mut q = album();
    q.apply(&Edit::Next).unwrap();
    q.apply(&Edit::Next).unwrap();
    q.apply(&Edit::Previous).unwrap();
    assert_eq!(q.current().as_deref(), Some("a.1"));
    assert_eq!(history(&q), ["a.0"]);
    assert_eq!(upcoming(&q), ["a.2", "a.3", "a.4"]);
    q.apply(&Edit::Next).unwrap();
    assert_eq!(q.current().as_deref(), Some("a.2"));
}

#[test]
fn previous_returns_a_manual_item_to_the_manual_queue() {
    let mut q = album();
    add(&mut q, source("m", 1), Anchor::ManualEnd);
    q.apply(&Edit::Next).unwrap();
    q.apply(&Edit::Previous).unwrap();
    assert_eq!(q.current().as_deref(), Some("a.0"));
    assert_eq!(manual(&q), ["m.0"]);
    q.apply(&Edit::Previous).unwrap();
    assert_eq!(q.current().as_deref(), Some("a.0"));
}

#[test]
fn vanished_items_are_ignored() {
    let mut q = album();
    let fresh = q.clone();
    remove(&mut q, &["nope.1", "a.9", "a.0"]);
    mv(&mut q, &["nope.1", "a.0"], Anchor::ManualStart).unwrap();
    assert_eq!(q, fresh);
    remove(&mut q, &["a.2"]);
    remove(&mut q, &["a.2"]);
    assert_eq!(upcoming(&q), ["a.1", "a.3", "a.4"]);
}

#[test]
fn vanished_anchors_fail_the_edit_and_change_nothing() {
    let mut q = album();
    remove(&mut q, &["a.2"]);
    let fresh = q.clone();
    let gone = || before("a.2");
    assert_eq!(mv(&mut q, &["a.3"], gone()), Err(Error::AnchorGone));
    let edit = Edit::Add { source: source("b", 1), before: gone() };
    assert_eq!(q.apply(&edit), Err(Error::AnchorGone));
    assert_eq!(mv(&mut q, &["a.3"], before("nope.0")), Err(Error::AnchorGone));
    assert_eq!(q, fresh);
}

#[test]
fn large_queue_edits_are_fast() {
    const N: usize = 500_000;
    let bound =
        if cfg!(debug_assertions) { Duration::from_secs(5) } else { Duration::from_millis(50) };
    let mut q = Queue::start(source("big", N), 0).unwrap();
    add(&mut q, source("m", 3), Anchor::ManualEnd);

    let started = Instant::now();
    mv(&mut q, &["big.400000", "big.300000"], before("big.2")).unwrap();
    assert!(started.elapsed() < bound, "move took {:?}", started.elapsed());
    assert_eq!(q.upcoming().take(3).collect::<Vec<_>>(), ["big.1", "big.300000", "big.400000"]);

    let started = Instant::now();
    jump(&mut q, "big.450000");
    assert!(started.elapsed() < bound, "jump took {:?}", started.elapsed());
    assert_eq!(q.current().as_deref(), Some("big.450000"));
    assert_eq!(history(&q).len(), 1 + 3 + 449_999);
}

#[test]
fn serde_round_trips_state_and_edits() {
    let mut q = album();
    add(&mut q, source("m", 2), Anchor::ManualEnd);
    mv(&mut q, &["a.4"], Anchor::ManualStart).unwrap();
    q.apply(&Edit::Next).unwrap();
    let json = serde_json::to_string(&q).unwrap();
    assert_eq!(serde_json::from_str::<Queue>(&json).unwrap(), q);

    let edits = [
        Edit::Add { source: source("b", 1), before: before("a.2") },
        Edit::Remove { items: strings(&["a.1"]) },
        Edit::Move { items: strings(&["a.1"]), before: Anchor::ContextEnd },
        Edit::Jump { item: "a.2".into() },
        Edit::Next,
        Edit::Previous,
    ];
    for edit in edits {
        let json = serde_json::to_string(&edit).unwrap();
        assert_eq!(serde_json::from_str::<Edit>(&json).unwrap(), edit);
    }
    let json =
        serde_json::to_value(Edit::Move { items: strings(&["a.1"]), before: Anchor::ManualStart })
            .unwrap();
    assert_eq!(
        json,
        serde_json::json!({"type": "move", "items": ["a.1"], "before": {"type": "manualStart"}})
    );
}

#[test]
fn previous_after_a_history_jump_does_not_duplicate_the_item_in_the_context() {
    let mut q = album();
    q.apply(&Edit::Next).unwrap();
    q.apply(&Edit::Next).unwrap();
    jump(&mut q, "a.0");
    q.apply(&Edit::Previous).unwrap();
    assert_eq!(q.current().as_deref(), Some("a.2"));
    assert_eq!(manual(&q), ["a.0"]);
    mv(&mut q, &["a.0"], Anchor::ContextEnd).unwrap();
    assert_eq!(upcoming(&q), ["a.3", "a.4", "a.0"]);
    assert_eq!(q.order.iter().filter(|&&i| i == 0).count(), 1);
}

#[test]
fn move_forgets_a_history_item_just_before_the_cursor() {
    let mut q = album();
    add(&mut q, source("m", 1), Anchor::ManualEnd);
    q.apply(&Edit::Next).unwrap();
    mv(&mut q, &["a.0"], Anchor::ContextEnd).unwrap();
    assert_eq!(q.order, [1, 2, 3, 4, 0]);
    assert_eq!(q.cursor, 0);
}

/// Turn shuffle on with `seed`, expecting success.
fn shuffle_on(q: &mut Queue, seed: u32) {
    q.apply(&Edit::Shuffle { on: true, seed }).unwrap();
}

/// Turn shuffle off, expecting success.
fn shuffle_off(q: &mut Queue) {
    q.apply(&Edit::Shuffle { on: false, seed: 0 }).unwrap();
}

/// Play the next item, expecting success.
fn next(q: &mut Queue) {
    q.apply(&Edit::Next).unwrap();
}

/// Item IDs `<key>.<n>` for each number.
fn ids_of(key: &str, numbers: &[u32]) -> Vec<String> {
    numbers.iter().map(|n| format!("{key}.{n}")).collect()
}

#[test]
fn shuffle_leaves_manual_passed_and_current_alone() {
    let mut q = Queue::start(source("a", 10), 3).unwrap();
    add(&mut q, source("m", 3), Anchor::ManualEnd);
    shuffle_on(&mut q, 7);
    assert_eq!(q.current().as_deref(), Some("a.3"));
    assert_eq!(manual(&q), ["m.0", "m.1", "m.2"]);
    assert_eq!(q.order[..4], [0, 1, 2, 3]);
    let mut tail = q.order[4..].to_vec();
    assert_ne!(tail, [4, 5, 6, 7, 8, 9]);
    tail.sort_unstable();
    assert_eq!(tail, [4, 5, 6, 7, 8, 9]);
}

#[test]
fn shuffle_when_already_shuffled_or_unshuffle_when_not_does_nothing() {
    let mut q = album();
    let plain = q.clone();
    shuffle_off(&mut q);
    assert_eq!(q, plain);
    shuffle_on(&mut q, 1);
    let shuffled = q.clone();
    shuffle_on(&mut q, 2);
    assert_eq!(q, shuffled);
}

#[test]
fn unshuffle_restores_the_original_order() {
    let mut q = Queue::start(source("a", 10), 0).unwrap();
    let plain = q.clone();
    shuffle_on(&mut q, 99);
    assert_ne!(upcoming(&q), upcoming(&plain));
    shuffle_off(&mut q);
    assert_eq!(q, plain);
}

#[test]
fn unshuffle_continues_from_the_current_items_natural_place() {
    let mut q = Queue::start(source("a", 10), 0).unwrap();
    shuffle_on(&mut q, 5);
    for _ in 0..3 {
        next(&mut q);
    }
    let played: Vec<u32> = q.history.clone();
    let current = q.current.unwrap();
    shuffle_off(&mut q);
    assert!(!q.shuffled());
    let expected: Vec<u32> = (current + 1..10).filter(|n| !played.contains(n)).collect();
    assert_eq!(upcoming(&q), ids_of("a", &expected));
    assert_eq!(q.current, Some(current));
    assert_eq!(q.order[q.cursor - 1], current);
    assert_eq!(q.history, played);
}

#[test]
fn unshuffle_acts_on_the_right_copy_of_a_duplicated_track() {
    let tracks = vec!["t".to_string(); 6];
    let dup = Source { key: "d".into(), library_id: "lib".into(), tracks };
    let mut q = Queue::start(dup, 0).unwrap();
    shuffle_on(&mut q, 11);
    next(&mut q);
    next(&mut q);
    let current = q.current.unwrap();
    let played = q.history.clone();
    shuffle_off(&mut q);
    let expected: Vec<u32> = (current + 1..6).filter(|n| !played.contains(n)).collect();
    assert_eq!(upcoming(&q), ids_of("d", &expected));
}

#[test]
fn unshuffle_reverts_moves_made_while_shuffled() {
    let mut q = Queue::start(source("a", 8), 0).unwrap();
    let plain = q.clone();
    shuffle_on(&mut q, 3);
    let first = upcoming(&q).remove(0);
    mv(&mut q, &[&first], Anchor::ContextEnd).unwrap();
    shuffle_off(&mut q);
    assert_eq!(q, plain);
}

#[test]
fn unshuffle_keeps_removals() {
    let mut q = Queue::start(source("a", 8), 0).unwrap();
    shuffle_on(&mut q, 3);
    remove(&mut q, &["a.4", "a.6"]);
    shuffle_off(&mut q);
    assert_eq!(upcoming(&q), ["a.1", "a.2", "a.3", "a.5", "a.7"]);
}

#[test]
fn unshuffle_keeps_additions_at_their_distance_from_the_current_item() {
    let mut q = Queue::start(source("a", 8), 0).unwrap();
    shuffle_on(&mut q, 3);
    let third = upcoming(&q)[2].clone();
    add(&mut q, source("x", 2), before(&third));
    add(&mut q, source("y", 1), Anchor::ContextEnd);
    assert_eq!(upcoming(&q)[2..4], ["x.0", "x.1"]);
    shuffle_off(&mut q);
    assert_eq!(
        upcoming(&q),
        ["a.1", "a.2", "x.0", "x.1", "a.3", "a.4", "a.5", "a.6", "a.7", "y.0"]
    );
}

#[test]
fn unshuffle_from_a_manual_item_resumes_after_the_last_context_item_started() {
    let mut q = Queue::start(source("a", 6), 0).unwrap();
    shuffle_on(&mut q, 8);
    next(&mut q);
    let last_started = q.current().unwrap();
    add(&mut q, source("m", 1), Anchor::ManualStart);
    next(&mut q);
    assert_eq!(q.current().as_deref(), Some("m.0"));
    let number: u32 = last_started.split('.').next_back().unwrap().parse().unwrap();
    shuffle_off(&mut q);
    let expected: Vec<u32> = (number + 1..6).collect();
    assert_eq!(upcoming(&q), ids_of("a", &expected));
    assert_eq!(history(&q), ["a.0".to_string(), last_started]);
}

#[test]
fn unshuffle_keeps_the_context_a_valid_queue() {
    let mut q = Queue::start(source("a", 10), 0).unwrap();
    shuffle_on(&mut q, 21);
    next(&mut q);
    next(&mut q);
    shuffle_off(&mut q);
    let mut seen = q.order.clone();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), q.order.len());
    assert!(q.cursor <= q.order.len());
}

#[test]
fn repeat_mode_is_recorded() {
    let mut q = album();
    assert_eq!(q.repeat(), Repeat::Off);
    q.apply(&Edit::Repeat { mode: Repeat::One }).unwrap();
    assert_eq!(q.repeat(), Repeat::One);
    next(&mut q);
    assert_eq!(q.current().as_deref(), Some("a.1"));
}

/// Item IDs and tracks of the current item followed by Up Next (manual first).
fn run(q: &Queue) -> Vec<(String, String)> {
    let ids = [q.current()].into_iter().flatten().chain(q.manual()).chain(q.upcoming());
    ids.map(|id| (id.clone(), q.track(&id).unwrap().track_id.to_string())).collect()
}

/// Play on until the current item comes from a restart copy (key starting with `~`).
fn play_into_next_lap(q: &mut Queue) {
    while !q.current().is_some_and(|id| id.starts_with('~')) {
        next(q);
    }
}

#[test]
fn repeat_all_restarts_into_copies_and_history_keeps_the_session() {
    let mut q = Queue::start(source("a", 3), 0).unwrap();
    add(&mut q, source("m", 1), Anchor::ManualEnd);
    q.apply(&Edit::Repeat { mode: Repeat::All }).unwrap();
    for _ in 0..3 {
        next(&mut q);
    }
    assert_eq!(q.current().as_deref(), Some("a.2"));
    next(&mut q);
    assert_eq!(q.current().as_deref(), Some("~0.0"));
    assert_eq!(upcoming(&q), ["~0.1", "~0.2"]);
    assert_eq!(history(&q), ["a.0", "m.0", "a.1", "a.2"]);
    assert_eq!(q.track("~0.2").map(|t| t.track_id), Some("a-2"));
}

#[test]
fn repeat_all_previous_on_a_laps_first_track_returns_to_the_last_lap() {
    let mut q = Queue::start(source("a", 2), 0).unwrap();
    q.apply(&Edit::Repeat { mode: Repeat::All }).unwrap();
    play_into_next_lap(&mut q);
    q.apply(&Edit::Previous).unwrap();
    assert_eq!(q.current().as_deref(), Some("a.1"));
    assert_eq!(upcoming(&q), ["~0.0", "~0.1"]);
    next(&mut q);
    assert_eq!(q.current().as_deref(), Some("~0.0"));
}

#[test]
fn repeat_all_laps_keep_restarting() {
    let mut q = Queue::start(source("a", 2), 0).unwrap();
    q.apply(&Edit::Repeat { mode: Repeat::All }).unwrap();
    for _ in 0..4 {
        next(&mut q);
    }
    assert_eq!(q.current().as_deref(), Some("~1.0"));
    assert_eq!(upcoming(&q), ["~1.1"]);
    assert_eq!(history(&q), ["a.0", "a.1", "~0.0", "~0.1"]);
}

#[test]
fn repeat_all_copies_a_run_per_library() {
    let mut q = Queue::start(source("a", 1), 0).unwrap();
    let other = Source { library_id: "other".into(), ..source("b", 2) };
    add(&mut q, other, Anchor::ContextEnd);
    add(&mut q, source("c", 1), Anchor::ContextEnd);
    q.apply(&Edit::Repeat { mode: Repeat::All }).unwrap();
    play_into_next_lap(&mut q);
    let libraries: Vec<&str> =
        ["~0.0", "~1.0", "~1.1", "~2.0"].iter().map(|id| q.track(id).unwrap().library_id).collect();
    assert_eq!(libraries, ["lib", "other", "other", "lib"]);
}

#[test]
fn repeat_all_replays_a_shuffled_order_without_reshuffling() {
    let mut q = Queue::start(source("a", 6), 0).unwrap();
    shuffle_on(&mut q, 17);
    q.apply(&Edit::Repeat { mode: Repeat::All }).unwrap();
    let lap: Vec<String> = run(&q).into_iter().map(|(_, track)| track).collect();
    play_into_next_lap(&mut q);
    let second: Vec<String> = run(&q).into_iter().map(|(_, track)| track).collect();
    assert_eq!(second, lap);
    assert!(q.shuffled());
}

#[test]
fn repeat_all_unshuffle_works_in_the_next_lap() {
    let mut q = Queue::start(source("a", 6), 0).unwrap();
    shuffle_on(&mut q, 17);
    q.apply(&Edit::Repeat { mode: Repeat::All }).unwrap();
    play_into_next_lap(&mut q);
    next(&mut q);
    shuffle_off(&mut q);
    let (id, track) = run(&q).remove(0);
    let number: usize = track.rsplit('-').next().unwrap().parse().unwrap();
    assert!(id.starts_with('~'));
    let expected: Vec<String> = (number + 1..6).map(|n| format!("a-{n}")).collect();
    let tracks: Vec<String> = run(&q).into_iter().skip(1).map(|(_, t)| t).collect();
    assert_eq!(tracks, expected);
}

#[test]
fn unshuffle_then_repeat_all_still_plays_every_track() {
    let mut q = Queue::start(source("a", 10), 0).unwrap();
    shuffle_on(&mut q, 42);
    next(&mut q);
    next(&mut q);
    shuffle_off(&mut q);
    q.apply(&Edit::Repeat { mode: Repeat::All }).unwrap();
    play_into_next_lap(&mut q);
    let mut tracks: Vec<String> = run(&q).into_iter().map(|(_, t)| t).collect();
    tracks.sort();
    let mut all: Vec<String> = (0..10).map(|n| format!("a-{n}")).collect();
    all.sort();
    assert_eq!(tracks, all);
}

#[test]
fn unshuffle_keeps_additions_when_the_base_runs_short() {
    let mut q = Queue::start(source("a", 8), 0).unwrap();
    shuffle_on(&mut q, 3);
    add(&mut q, source("y", 1), Anchor::ContextEnd);
    let two: Vec<String> = upcoming(&q).into_iter().take(2).collect();
    q.apply(&Edit::Remove { items: two }).unwrap();
    shuffle_off(&mut q);
    assert_eq!(q.upcoming().last().as_deref(), Some("y.0"));
    assert_eq!(upcoming(&q).len(), 6);
}

#[test]
fn unshuffle_from_a_former_context_item_playing_from_the_manual_queue() {
    let mut q = Queue::start(source("a", 6), 0).unwrap();
    shuffle_on(&mut q, 8);
    next(&mut q);
    let last_started: u32 = q.current().unwrap().rsplit('.').next().unwrap().parse().unwrap();
    let moved = upcoming(&q)[0].clone();
    mv(&mut q, &[&moved], Anchor::ManualStart).unwrap();
    next(&mut q);
    assert_eq!(q.current(), Some(moved.clone()));
    shuffle_off(&mut q);
    let expected: Vec<u32> = (last_started + 1..6).filter(|n| format!("a.{n}") != moved).collect();
    assert_eq!(upcoming(&q), ids_of("a", &expected));
}

#[test]
fn repeat_off_ends_at_the_end() {
    let mut q = Queue::start(source("a", 2), 0).unwrap();
    next(&mut q);
    next(&mut q);
    assert_eq!(q.current(), None);
}

#[test]
fn large_queue_shuffle_is_fast() {
    let bound =
        if cfg!(debug_assertions) { Duration::from_secs(5) } else { Duration::from_millis(50) };
    let mut q = Queue::start(source("big", 500_000), 0).unwrap();
    let started = Instant::now();
    shuffle_on(&mut q, 1);
    assert!(started.elapsed() < bound, "shuffle took {:?}", started.elapsed());
    let started = Instant::now();
    shuffle_off(&mut q);
    assert!(started.elapsed() < bound, "unshuffle took {:?}", started.elapsed());
    assert_eq!(q.order.len(), 500_000);
    assert_eq!(q.order[1], 1);
}

#[test]
fn serde_round_trips_shuffle_and_repeat() {
    let mut q = album();
    shuffle_on(&mut q, 9);
    q.apply(&Edit::Repeat { mode: Repeat::All }).unwrap();
    let json = serde_json::to_value(&q).unwrap();
    assert_eq!(json["repeat"], "all");
    assert_eq!(json["shuffle"]["seed"], 9);
    assert!(json["shuffle"]["unshuffled"].is_array());
    assert_eq!(serde_json::from_value::<Queue>(json).unwrap(), q);

    assert_eq!(
        serde_json::to_value(Edit::Shuffle { on: true, seed: 5 }).unwrap(),
        serde_json::json!({"type": "shuffle", "on": true, "seed": 5})
    );
    assert_eq!(
        serde_json::to_value(Edit::Repeat { mode: Repeat::One }).unwrap(),
        serde_json::json!({"type": "repeat", "mode": "one"})
    );
}

#[test]
fn shuffle_golden_queue_order_follows_the_pinned_permutation() {
    let mut q = Queue::start(source("a", 11), 0).unwrap();
    shuffle_on(&mut q, 42);
    assert_eq!(q.order, [0, 1, 10, 6, 9, 7, 5, 8, 3, 2, 4]);
    assert_eq!(q.shuffle.as_ref().map(|s| s.seed), Some(42));
}

#[test]
fn unshuffle_places_additions_after_a_base_that_ran_out() {
    let mut q = Queue::start(source("a", 8), 0).unwrap();
    shuffle_on(&mut q, 3);
    jump(&mut q, "a.7");
    add(&mut q, source("y", 1), Anchor::ContextEnd);
    shuffle_off(&mut q);
    assert_eq!(upcoming(&q), ["y.0"]);
}

#[test]
fn reserved_keys_are_refused_and_change_nothing() {
    assert_eq!(Queue::start(source("~0", 2), 0), Err(Error::ReservedKey));
    let mut q = album();
    let edit = Edit::Add { source: source("~7", 1), before: Anchor::ManualEnd };
    assert_eq!(q.apply(&edit), Err(Error::ReservedKey));
    assert_eq!(q, album());
}
