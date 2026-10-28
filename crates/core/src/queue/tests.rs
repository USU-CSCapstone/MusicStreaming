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
