//! Tests of Up Next edits and clean-up, and the invariant checker every queue test can use.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::permutation::permute;
use super::*;

/// A source of `n` tracks keyed `key` in library `lib`.
fn source(key: &str, n: u64) -> Source {
    Source::new(key, "lib", n, None)
}

/// A queue playing source `a` of five tracks from its first.
fn album() -> QueueState {
    let mut state = QueueState::default();
    state.start(source("a", 5), 0).unwrap();
    state
}

/// The item `<key>.<pos>`.
fn item(id: &str) -> ItemId {
    id.parse().unwrap()
}

/// The items `ids`.
fn items(ids: &[&str]) -> Vec<ItemId> {
    ids.iter().map(|id| item(id)).collect()
}

/// An anchor just ahead of `id`.
fn before(id: &str) -> Anchor {
    Anchor::Before { item: item(id) }
}

/// Applies `edit` and checks the invariants after it.
fn apply(state: &mut QueueState, edit: Edit) -> Result<(), QueueError> {
    let result = state.apply(&edit);
    assert_invariants(state);
    result
}

/// Adds `source` at `at`, expecting success.
fn add(state: &mut QueueState, source: Source, at: Anchor) {
    apply(state, Edit::Add { source, before: at }).unwrap();
}

/// Moves `ids` to `at`.
fn mv(state: &mut QueueState, ids: &[&str], at: Anchor) -> Result<(), QueueError> {
    apply(state, Edit::Move { items: items(ids), before: at })
}

/// Removes `ids`.
fn remove(state: &mut QueueState, ids: &[&str]) {
    apply(state, Edit::Remove { items: items(ids) }).unwrap();
}

/// Plays the next item.
fn next(state: &mut QueueState) {
    apply(state, Edit::Next).unwrap();
}

/// Jumps to `id`.
fn jump(state: &mut QueueState, id: &str) {
    apply(state, Edit::Jump { item: item(id) }).unwrap();
}

/// A play reference.
fn play(no: u64, id: &str) -> PlayRef {
    PlayRef { play: no, item: item(id) }
}

/// The plays in History, oldest first, as (number, item).
fn plays(state: &QueueState) -> Vec<(u64, ItemId)> {
    state
        .history
        .iter()
        .flat_map(|run| {
            (0..run.span.len).map(move |i| {
                let View::Key(key) = &run.span.view else { panic!("a run reads the shuffle") };
                (run.first + i, ItemId { key: key.clone(), pos: run.span.start + i })
            })
        })
        .collect()
}

/// The Manual Queue's items.
fn manual(state: &QueueState) -> Vec<String> {
    (0..state.manual.len()).map(|i| state.up_next_row(i).unwrap().item.to_string()).collect()
}

/// The context's Up Next items.
fn upcoming(state: &QueueState) -> Vec<String> {
    (state.manual.len()..state.up_next_len())
        .map(|i| state.up_next_row(i).unwrap().item.to_string())
        .collect()
}

/// The playing item.
fn current(state: &QueueState) -> Option<String> {
    state.current.as_ref().map(ToString::to_string)
}

/// The number of spans in the Manual Queue and the context.
fn span_count(state: &QueueState) -> usize {
    state.manual.spans().len() + state.order.spans().len()
}

/// The items read by every row of `list`.
fn list_items(list: &SpanList) -> Vec<ItemId> {
    list.spans()
        .iter()
        .flat_map(|s| {
            (s.start..s.start + s.len).filter_map(move |pos| match &s.view {
                View::Key(key) => Some(ItemId { key: key.clone(), pos }),
                View::Shuffle => None,
            })
        })
        .collect()
}

/// The item every row of the context reads, shuffle rows included.
pub(super) fn order_items(state: &QueueState) -> Vec<ItemId> {
    (0..state.order.len()).map(|row| state.item_at(&state.order, row).unwrap()).collect()
}

/// Checks what must hold after every edit: keys unique, every span reads a real row of a real
/// view, each item is in Up Next at most once and in `order` at most once, the playing item
/// isn't in Up Next, no source or copy is unreferenced, and while shuffled the shuffle rows
/// are distinct positions of the base.
pub(super) fn assert_invariants(state: &QueueState) {
    let mut keys = HashSet::new();
    for key in state.sources.iter().map(|s| &s.key).chain(state.copies.iter().map(|c| &c.key)) {
        assert!(keys.insert(key.as_str()), "key {key} is used twice");
    }

    let length_of = |key: &str| -> Option<u64> {
        state
            .sources
            .iter()
            .find(|s| s.key == key)
            .map(|s| s.length)
            .or_else(|| state.copies.iter().find(|c| c.key == key).map(|c| c.of.len()))
    };
    let mut referenced: HashSet<String> = HashSet::new();
    let mut check = |list: &SpanList, what: &str| {
        for span in list.spans() {
            match &span.view {
                View::Key(key) => {
                    let len = length_of(key).unwrap_or_else(|| panic!("{what} reads gone {key}"));
                    assert!(span.start + span.len <= len, "{what} reads past the end of {key}");
                    referenced.insert(key.clone());
                }
                View::Shuffle => assert!(state.shuffle.is_some(), "{what} reads a missing shuffle"),
            }
        }
    };
    check(&state.manual, "manual");
    assert!(
        state.manual.spans().iter().all(|s| s.view != View::Shuffle),
        "the Manual Queue reads the shuffle"
    );
    check(&state.order, "order");
    let mut shuffle_rows = HashSet::new();
    for span in state.order.spans().iter().filter(|s| s.view == View::Shuffle) {
        let m = state.shuffle.as_ref().map_or(0, |s| s.base.len());
        assert!(span.start + span.len <= m, "order reads past the shuffle");
        for p in span.start..span.start + span.len {
            assert!(shuffle_rows.insert(p), "shuffle row {p} is in order twice");
        }
    }
    if let Some(shuffle) = &state.shuffle {
        check(&shuffle.base, "shuffle base");
    }
    for run in &state.history {
        check(&SpanList::from_spans([run.span.clone()]), "history");
    }
    for copy in &state.copies {
        for span in copy.of.spans() {
            let View::Key(key) = &span.view else { panic!("copy {} reads a shuffle", copy.key) };
            let source = state.sources.iter().find(|s| s.key == *key);
            let source =
                source.unwrap_or_else(|| panic!("copy {} doesn't name a source", copy.key));
            assert!(span.start + span.len <= source.length, "copy reads past its source");
        }
    }
    if let Some(current) = &state.current {
        let len = length_of(&current.key).expect("current names a gone view");
        assert!(current.pos < len, "current is past the end");
        referenced.insert(current.key.clone());
    }

    // A copy keeps the sources it names; anything else unreferenced should have gone.
    let named: Vec<String> = state
        .copies
        .iter()
        .filter(|c| referenced.contains(&c.key))
        .flat_map(|c| list_items(&c.of).into_iter().map(|i| i.key))
        .collect();
    referenced.extend(named);
    for key in &keys {
        assert!(referenced.contains(*key), "{key} is unreferenced");
    }

    assert!(state.history.len() <= HISTORY_RUNS, "History keeps too many runs");
    let mut next_free = 0;
    for run in &state.history {
        assert!(run.span.len > 0, "an empty run");
        assert!(run.first >= next_free, "runs overlap or are out of order");
        assert!(matches!(run.span.view, View::Key(_)), "a run reads the shuffle");
        next_free = run.first + run.span.len;
    }
    assert!(next_free <= state.next_play, "a play number is past nextPlay");

    assert!(state.cursor <= state.order.len(), "cursor past the context");
    let passed = order_items(state);
    let unique: HashSet<_> = passed.iter().collect();
    assert_eq!(unique.len(), passed.len(), "an item is in order twice");
    let mut up_next = list_items(&state.manual);
    up_next.extend(passed[state.cursor as usize..].iter().cloned());
    let unique: HashSet<_> = up_next.iter().collect();
    assert_eq!(unique.len(), up_next.len(), "an item is in Up Next twice");
    if let Some(current) = &state.current {
        assert!(!unique.contains(current), "the playing item is in Up Next");
    }
    // A Manual Queue item also in `order` is a two-place track: its place there is passed.
    for row in list_items(&state.manual) {
        if let Some(at) = passed.iter().position(|i| *i == row) {
            assert!((at as u64) < state.cursor, "{row} holds a second place in Up Next");
        }
    }
}

#[test]
fn start_state_holds_the_invariants() {
    assert_invariants(&album());
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
    add(&mut q, source("m", 1), Anchor::ManualEnd);
    let fresh = q.clone();
    let edit = |key: &str| Edit::Add { source: source(key, 1), before: Anchor::ManualEnd };
    assert_eq!(apply(&mut q, edit("a")), Err(QueueError::DuplicateKey));
    assert_eq!(apply(&mut q, edit("m")), Err(QueueError::DuplicateKey));
    assert_eq!(q, fresh);
}

#[test]
fn add_of_an_empty_source_changes_nothing() {
    let mut q = album();
    let fresh = q.clone();
    add(&mut q, source("e", 0), Anchor::ManualEnd);
    assert_eq!(q, fresh);
}

#[test]
fn remove_from_up_next() {
    let mut q = album();
    add(&mut q, source("m", 2), Anchor::ManualEnd);
    remove(&mut q, &["m.0", "a.3"]);
    assert_eq!(manual(&q), ["m.1"]);
    assert_eq!(upcoming(&q), ["a.1", "a.2", "a.4"]);
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
fn move_cannot_use_its_own_item_or_a_non_upcoming_item_as_anchor() {
    let mut q = album();
    next(&mut q);
    let fresh = q.clone();
    assert_eq!(mv(&mut q, &["a.2", "a.3"], before("a.3")), Err(QueueError::AnchorGone));
    assert_eq!(mv(&mut q, &["a.2"], before("a.0")), Err(QueueError::AnchorGone));
    assert_eq!(mv(&mut q, &["a.2"], before("a.1")), Err(QueueError::AnchorGone));
    assert_eq!(q, fresh);
}

#[test]
fn jump_in_up_next_moves_everything_ahead_out() {
    let mut q = album();
    add(&mut q, source("m", 2), Anchor::ManualEnd);
    jump(&mut q, "a.3");
    assert_eq!(current(&q), Some("a.3".into()));
    assert!(manual(&q).is_empty());
    assert_eq!(upcoming(&q), ["a.4"]);
    assert_eq!(q.cursor, 4);
    // The Manual Queue's source stays while History names it.
    assert_eq!(q.sources.len(), 2);
}

#[test]
fn jump_within_the_manual_queue_leaves_the_context() {
    let mut q = album();
    add(&mut q, source("m", 3), Anchor::ManualEnd);
    jump(&mut q, "m.1");
    assert_eq!(current(&q), Some("m.1".into()));
    assert_eq!(manual(&q), ["m.2"]);
    assert_eq!(upcoming(&q), ["a.1", "a.2", "a.3", "a.4"]);
    assert_eq!(q.cursor, 1);
}

#[test]
fn jump_to_the_current_item_or_a_gone_item_does_nothing() {
    let mut q = album();
    let fresh = q.clone();
    jump(&mut q, "a.0");
    jump(&mut q, "zzz.9");
    jump(&mut q, "a.9");
    assert_eq!(q, fresh);
}

#[test]
fn next_plays_manual_first_then_the_context_and_ends() {
    let mut q = QueueState::default();
    q.start(source("a", 2), 0).unwrap();
    add(&mut q, source("m", 1), Anchor::ManualEnd);
    let mut played = vec![current(&q).unwrap()];
    for _ in 0..2 {
        next(&mut q);
        played.extend(current(&q));
    }
    assert_eq!(played, ["a.0", "m.0", "a.1"]);
    next(&mut q);
    assert_eq!(current(&q), None);
    next(&mut q);
    assert_eq!(current(&q), None);
    assert_eq!(q.up_next_len(), 0);
}

/// A run as (first play, view key, start, length).
fn runs(state: &QueueState) -> Vec<(u64, String, u64, u64)> {
    state
        .history
        .iter()
        .map(|r| {
            let View::Key(key) = &r.span.view else { panic!("a run reads the shuffle") };
            (r.first, key.clone(), r.span.start, r.span.len)
        })
        .collect()
}

#[test]
fn album_played_through_is_one_run() {
    let mut q = album();
    for _ in 0..4 {
        next(&mut q);
    }
    // The current item is not a play yet.
    assert_eq!(runs(&q), [(0, "a".into(), 0, 4)]);
    assert_eq!((q.next_play, q.history_len()), (4, 4));
    next(&mut q);
    assert_eq!(runs(&q), [(0, "a".into(), 0, 5)]);
    assert_eq!(current(&q), None);
    let newest = q.history_row(0).unwrap();
    assert_eq!((newest.play, newest.row.item), (4, item("a.4")));
    assert_eq!(q.history_row(4).unwrap().play, 0);
    assert!(q.history_row(5).is_none());
    assert!(!q.adjusted);
}

#[test]
fn long_jump_is_one_run() {
    let mut q = QueueState::default();
    q.start(source("a", 500), 0).unwrap();
    remove(&mut q, &["a.10", "a.20"]);
    jump(&mut q, "a.400");
    // The item playing and everything skipped: one run per span passed.
    assert_eq!(
        runs(&q),
        [(0, "a".into(), 0, 10), (10, "a".into(), 11, 9), (19, "a".into(), 21, 379)]
    );
    assert_eq!(q.next_play, 398);

    // Manual Queue rows ahead of the target are plays too, ahead of the context's.
    let mut q = album();
    add(&mut q, source("m", 2), Anchor::ManualEnd);
    jump(&mut q, "a.2");
    assert_eq!(
        plays(&q).into_iter().map(|(_, i)| i.to_string()).collect::<Vec<_>>(),
        ["a.0", "m.0", "m.1", "a.1"]
    );
    // A jump within the Manual Queue plays what it passes.
    let mut q = album();
    add(&mut q, source("m", 3), Anchor::ManualEnd);
    jump(&mut q, "m.1");
    assert_eq!(
        plays(&q).into_iter().map(|(_, i)| i.to_string()).collect::<Vec<_>>(),
        ["a.0", "m.0"]
    );
}

#[test]
fn history_keeps_newest_runs() {
    let mut q = album();
    // Applied directly: the invariant check after each of 2,200 edits would dominate.
    for i in 0..1_100 {
        let source = source(&format!("m{i}"), 1);
        q.apply(&Edit::Add { source, before: Anchor::ManualEnd }).unwrap();
    }
    for _ in 0..1_100 {
        q.apply(&Edit::Next).unwrap();
    }
    assert_invariants(&q);
    // Each source is its own run.
    assert_eq!(q.history.len(), HISTORY_RUNS);
    assert_eq!(q.next_play, 1_100);
    assert_eq!(q.history[0].first, 100);
    assert_eq!(q.history_row(0).unwrap().row.item, item("m1098.0"));
    assert_eq!(q.history_len(), HISTORY_RUNS as u64);
    // What the trimmed runs named is cleaned up with them.
    assert!(q.sources.iter().all(|s| s.key != "m0"));
    assert!(q.sources.iter().any(|s| s.key == "m1098"));
}

#[test]
fn history_stays_trimmed_when_a_removal_splits_a_run() {
    // 999 one-play runs, then one run of four plays: exactly 1,000 runs.
    let mut q = album();
    for i in 0..999 {
        let source = source(&format!("m{i}"), 1);
        q.apply(&Edit::Add { source, before: Anchor::ManualEnd }).unwrap();
    }
    for _ in 0..1_000 {
        q.apply(&Edit::Next).unwrap();
    }
    q.apply(&Edit::Add { source: source("z", 5), before: Anchor::ManualEnd }).unwrap();
    for _ in 0..5 {
        q.apply(&Edit::Next).unwrap();
    }
    assert_eq!(q.history.len(), HISTORY_RUNS);
    let last = q.history.last().unwrap().clone();
    assert_eq!(last.span.len, 4);

    // Removing a play from the middle of that run splits it, and the oldest run goes.
    let mid = play(last.first + 1, &format!("z.{}", last.span.start + 1));
    let plays_before = q.history_len();
    apply(&mut q, Edit::RemovePlays { plays: vec![mid] }).unwrap();
    assert_eq!(q.history.len(), HISTORY_RUNS);
    assert_eq!(q.history_len(), plays_before - 2);

    // So does a re-queue.
    let mid = play(last.first + 2, &format!("z.{}", last.span.start + 2));
    apply(&mut q, Edit::Requeue { play: mid, key: "c".into() }).unwrap();
    assert_eq!(q.history.len(), HISTORY_RUNS);
}

#[test]
fn jump_with_full_history_drops_the_trimmed_runs_source() {
    let mut q = album();
    q.apply(&Edit::Add { source: source("x", 1), before: Anchor::ManualStart }).unwrap();
    for i in 0..999 {
        let source = source(&format!("m{i}"), 1);
        q.apply(&Edit::Add { source, before: Anchor::ManualEnd }).unwrap();
    }
    // Plays a.0, x.0, m0 ... m997: 1,000 runs, and m998 is playing.
    for _ in 0..1_000 {
        q.apply(&Edit::Next).unwrap();
    }
    assert_eq!((q.history.len(), current(&q)), (HISTORY_RUNS, Some("m998.0".into())));
    for key in ["y", "z"] {
        q.apply(&Edit::Add { source: source(key, 1), before: Anchor::ManualEnd }).unwrap();
    }
    assert!(q.sources.iter().any(|s| s.key == "x"));

    // Jumping over y and z plays three runs, which trims a.0's and x.0's: x is named no more.
    jump(&mut q, "a.1");
    assert_eq!(q.history.len(), HISTORY_RUNS);
    assert!(q.sources.iter().all(|s| s.key != "x"));
}

#[test]
fn repeat_all_restarts_by_cursor() {
    let mut q = QueueState::default();
    q.start(source("a", 3), 0).unwrap();
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    for _ in 0..2 {
        next(&mut q);
    }
    let order = q.order.clone();
    next(&mut q);
    assert_eq!(current(&q), Some("a.0".into()));
    assert_eq!((q.cursor, &q.order), (1, &order));
    assert!(q.copies.is_empty() && q.sources.len() == 1);
    assert_eq!(upcoming(&q), ["a.1", "a.2"]);
    assert_eq!(runs(&q), [(0, "a".into(), 0, 3)]);

    // Repeat off ends instead, and Repeat One leaves Next to advance.
    let mut q = QueueState::default();
    q.start(source("a", 2), 0).unwrap();
    apply(&mut q, Edit::Repeat { mode: Repeat::One }).unwrap();
    next(&mut q);
    assert_eq!(current(&q), Some("a.1".into()));
    next(&mut q);
    assert_eq!(current(&q), None);
}

#[test]
fn stale_play_number_removes_nothing() {
    let mut q = album();
    for _ in 0..3 {
        next(&mut q);
    }
    let fresh = q.clone();
    let remove_plays = |q: &mut QueueState, ps| apply(q, Edit::RemovePlays { plays: ps });
    // The number is right and the item wrong, the item right and the number wrong, and a play
    // that never was.
    remove_plays(&mut q, vec![play(1, "a.2"), play(2, "a.1"), play(9, "a.9"), play(1, "x.1")])
        .unwrap();
    assert_eq!(q, fresh);
    assert!(!q.adjusted);

    remove_plays(&mut q, vec![play(1, "a.1"), play(1, "a.1")]).unwrap();
    assert_eq!(runs(&q), [(0, "a".into(), 0, 1), (2, "a".into(), 2, 1)]);
    assert!(q.adjusted);
    assert_eq!(q.next_play, 3);
    // The number is not reused, and the gap closes for reads.
    assert_eq!(q.history_row(0).unwrap().play, 2);
    assert_eq!(q.history_row(1).unwrap().play, 0);
    remove_plays(&mut q, vec![play(0, "a.0"), play(2, "a.2")]).unwrap();
    assert!(q.history.is_empty());
    next(&mut q);
    assert_eq!(q.history[0].first, 3);
    // The sources they named are dropped with the last play, unless still referenced.
    assert_eq!(q.sources.len(), 1);
}

#[test]
fn requeue_puts_copy_first() {
    let mut q = album();
    add(&mut q, source("m", 1), Anchor::ManualEnd);
    q.adjusted = false;
    for _ in 0..3 {
        next(&mut q);
    }
    // Plays 0, 1, 2 are a.0, m.0, a.1, and a.2 is playing.
    let fresh = q.clone();

    // A stale play changes nothing.
    apply(&mut q, Edit::Requeue { play: play(0, "a.1"), key: "c".into() }).unwrap();
    assert_eq!(q, fresh);
    assert!(!q.adjusted);

    apply(&mut q, Edit::Requeue { play: play(2, "a.1"), key: "c".into() }).unwrap();
    assert!(q.adjusted);
    assert_eq!(manual(&q), ["c.0"]);
    assert_eq!(q.copies.len(), 1);
    assert_eq!(q.copies[0].of.spans(), [Span::new(View::Key("a".into()), 1, 1)]);
    assert_eq!(runs(&q), [(0, "a".into(), 0, 1), (1, "m".into(), 0, 1)]);
    // The copy reads the source row, and the context's own item stays where it was.
    assert_eq!(q.up_next_row(0).unwrap().position, 1);
    assert_eq!(upcoming(&q), ["a.3", "a.4"]);

    // A key in use fails the edit.
    let edit = Edit::Requeue { play: play(1, "m.0"), key: "c".into() };
    let before = q.clone();
    assert_eq!(apply(&mut q, edit), Err(QueueError::DuplicateKey));
    assert_eq!(q, before);

    // The copy of a copy is flattened onto its source.
    next(&mut q);
    next(&mut q);
    assert_eq!(current(&q), Some("a.3".into()));
    apply(&mut q, Edit::Requeue { play: play(4, "c.0"), key: "d".into() }).unwrap();
    assert_eq!(q.copies.len(), 1);
    assert_eq!(q.copies[0].key, "d");
    assert_eq!(q.copies[0].of.spans(), [Span::new(View::Key("a".into()), 1, 1)]);
}

/// Applies `Previous` under `key`.
fn previous(state: &mut QueueState, key: &str) -> Result<(), QueueError> {
    apply(state, Edit::Previous { key: key.into() })
}

#[test]
fn previous_in_album_moves_cursor() {
    let mut q = album();
    next(&mut q);
    next(&mut q);
    let order = q.order.clone();
    previous(&mut q, "a").unwrap();
    assert_eq!(current(&q), Some("a.1".into()));
    assert_eq!((q.cursor, &q.order), (2, &order));
    assert_eq!(upcoming(&q), ["a.2", "a.3", "a.4"]);
    assert!(q.manual.is_empty() && q.copies.is_empty() && !q.adjusted);
    assert_eq!(runs(&q), [(0, "a".into(), 0, 1)]);
    // The number of the play taken back is never reused.
    next(&mut q);
    assert_eq!(runs(&q), [(0, "a".into(), 0, 1), (2, "a".into(), 1, 1)]);

    // At the first track History is empty, and nothing changes.
    let mut q = album();
    let fresh = q.clone();
    previous(&mut q, "c").unwrap();
    assert_eq!(q, fresh);

    // Nothing playing: the last play comes back, and Up Next stays as it was.
    let mut q = QueueState::default();
    q.start(source("a", 2), 0).unwrap();
    next(&mut q);
    next(&mut q);
    assert_eq!(current(&q), None);
    previous(&mut q, "c").unwrap();
    assert_eq!((current(&q), q.up_next_len()), (Some("a.1".into()), 0));
}

#[test]
fn previous_keeps_current_source() {
    let mut q = album();
    add(&mut q, source("m", 1), Anchor::ManualEnd);
    next(&mut q);
    next(&mut q);
    // Plays a.0 and m.0, and a.1 is playing: History's run of m is the only thing naming it.
    previous(&mut q, "c").unwrap();
    assert_eq!(current(&q), Some("m.0".into()));
    assert!(q.sources.iter().any(|s| s.key == "m"));
    assert_eq!(upcoming(&q), ["a.1", "a.2", "a.3", "a.4"]);
    // Dropping what the play named, when nothing else does, goes with the next step back.
    previous(&mut q, "d").unwrap();
    assert_eq!(current(&q), Some("a.0".into()));
    assert_eq!(upcoming(&q), ["a.1", "a.2", "a.3", "a.4"]);
    assert_eq!(manual(&q), ["m.0"]);
    assert!(q.history.is_empty());
}

#[test]
fn previous_after_restart_copies_track() {
    let mut q = QueueState::default();
    q.start(source("a", 3), 0).unwrap();
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    for _ in 0..3 {
        next(&mut q);
    }
    assert_eq!((current(&q), q.cursor), (Some("a.0".into()), 1));

    // A key in use fails the edit and changes nothing.
    let fresh = q.clone();
    assert_eq!(previous(&mut q, "a"), Err(QueueError::DuplicateKey));
    assert_eq!(q, fresh);

    // a.2 is in Up Next once the cursor goes back, so a copy of it plays and a.2 stays queued.
    previous(&mut q, "c").unwrap();
    assert_eq!(current(&q), Some("c.0".into()));
    assert_eq!((q.cursor, upcoming(&q)), (0, ["a.0", "a.1", "a.2"].map(String::from).to_vec()));
    assert_eq!(q.copies[0].of.spans(), [Span::new(View::Key("a".into()), 2, 1)]);
    assert_eq!(q.current_row().unwrap().position, 2);

    // One track on Repeat All: the cursor going back is what puts the track in Up Next.
    let mut q = QueueState::default();
    q.start(source("a", 1), 0).unwrap();
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    next(&mut q);
    previous(&mut q, "c").unwrap();
    assert_eq!((current(&q), upcoming(&q)), (Some("c.0".into()), vec!["a.0".to_string()]));
}

#[test]
fn previous_flattens_the_copy_of_a_copy() {
    let mut q = QueueState::default();
    q.start(source("a", 3), 0).unwrap();
    next(&mut q);
    next(&mut q);
    apply(&mut q, Edit::Requeue { play: play(1, "a.1"), key: "c".into() }).unwrap();
    mv(&mut q, &["c.0"], Anchor::ContextEnd).unwrap();
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    next(&mut q);
    next(&mut q);
    assert_eq!(current(&q), Some("a.0".into()));
    previous(&mut q, "d").unwrap();
    assert_eq!(current(&q), Some("d.0".into()));
    let d = q.copies.iter().find(|c| c.key == "d").unwrap();
    assert_eq!(d.of.spans(), [Span::new(View::Key("a".into()), 1, 1)]);
}

#[test]
fn two_place_track_lap_place() {
    // Plays a.0 and then a.1 with m.0 queued; Previous puts a.1 in the Manual Queue.
    let two_place = || {
        let mut q = album();
        next(&mut q);
        add(&mut q, source("m", 1), Anchor::ManualEnd);
        previous(&mut q, "c").unwrap();
        assert_eq!(
            (current(&q), manual(&q)),
            (Some("a.0".into()), vec!["a.1".into(), "m.0".into()])
        );
        // a.1 keeps its passed place, and Up Next still has it once.
        assert_eq!((q.cursor, q.find_item(&item("a.1"))), (2, Some(0)));
        q
    };
    // Plays out what is left of a lap, then the next lap's first tracks.
    let lap = |mut q: QueueState| {
        apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
        let mut seen = Vec::new();
        for _ in 0..8 {
            next(&mut q);
            seen.extend(current(&q));
        }
        seen
    };

    // Moved within the Manual Queue, it keeps the passed place: the next lap plays it.
    let mut q = two_place();
    mv(&mut q, &["a.1"], Anchor::ManualEnd).unwrap();
    assert_eq!((q.cursor, manual(&q)), (2, vec!["m.0".into(), "a.1".into()]));
    assert_eq!(lap(q), ["m.0", "a.1", "a.2", "a.3", "a.4", "a.0", "a.1", "a.2"]);

    // Moved into the context, it holds one place: ahead of a.3, and gone from lap start.
    let mut q = two_place();
    mv(&mut q, &["a.1"], before("a.3")).unwrap();
    assert_eq!(
        (q.cursor, upcoming(&q)),
        (1, ["a.2", "a.1", "a.3", "a.4"].map(String::from).to_vec())
    );
    assert_eq!(lap(q), ["m.0", "a.2", "a.1", "a.3", "a.4", "a.0", "a.2", "a.1"]);

    // Removed, it is gone from later laps too.
    let mut q = two_place();
    remove(&mut q, &["a.1"]);
    assert_eq!((q.cursor, manual(&q)), (1, vec!["m.0".to_string()]));
    assert_eq!(lap(q), ["m.0", "a.2", "a.3", "a.4", "a.0", "a.2", "a.3", "a.4"]);

    // Moved with something else out of the context, to the context's end.
    let mut q = two_place();
    mv(&mut q, &["a.1", "a.3"], Anchor::ContextEnd).unwrap();
    assert_eq!(
        (q.cursor, upcoming(&q)),
        (1, ["a.2", "a.4", "a.1", "a.3"].map(String::from).to_vec())
    );
}

#[test]
fn repeat_is_recorded() {
    let mut q = album();
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    assert_eq!(q.repeat, Repeat::All);
}

#[test]
fn edit_on_gone_item_is_ignored() {
    let mut q = album();
    let fresh = q.clone();
    remove(&mut q, &["nope.1", "a.9", "a.0"]);
    mv(&mut q, &["nope.1", "a.0"], Anchor::ManualStart).unwrap();
    assert_eq!(q, fresh);
    assert!(!q.adjusted);

    remove(&mut q, &["a.2"]);
    let removed = q.clone();
    remove(&mut q, &["a.2"]);
    assert_eq!(q, removed);
    assert_eq!(upcoming(&q), ["a.1", "a.3", "a.4"]);

    // An item of a dropped source is gone like any other.
    add(&mut q, source("m", 1), Anchor::ManualEnd);
    remove(&mut q, &["m.0"]);
    assert!(q.sources.iter().all(|s| s.key != "m"));
    let after = q.clone();
    remove(&mut q, &["m.0"]);
    mv(&mut q, &["m.0"], Anchor::ManualEnd).unwrap();
    jump(&mut q, "m.0");
    assert_eq!(q, after);
}

#[test]
fn vanished_anchors_fail_the_edit_and_change_nothing() {
    let mut q = album();
    remove(&mut q, &["a.2"]);
    let fresh = q.clone();
    assert_eq!(mv(&mut q, &["a.3"], before("a.2")), Err(QueueError::AnchorGone));
    let edit = Edit::Add { source: source("b", 1), before: before("a.2") };
    assert_eq!(apply(&mut q, edit), Err(QueueError::AnchorGone));
    assert_eq!(mv(&mut q, &["a.3"], before("nope.0")), Err(QueueError::AnchorGone));
    assert_eq!(mv(&mut q, &[], before("nope.0")), Err(QueueError::AnchorGone));
    assert_eq!(q, fresh);
}

#[test]
fn adjusted_only_by_user_edits() {
    let mut q = album();
    add(&mut q, source("m", 3), Anchor::ManualEnd);
    q.adjusted = false;
    next(&mut q);
    jump(&mut q, "a.2");
    apply(&mut q, Edit::Repeat { mode: Repeat::One }).unwrap();
    next(&mut q);
    assert!(!q.adjusted);

    for edit in [
        Edit::Add { source: source("x", 1), before: Anchor::ContextEnd },
        Edit::Remove { items: items(&["a.4"]) },
        Edit::Move { items: items(&["a.3"]), before: Anchor::ManualStart },
    ] {
        let mut q = album();
        apply(&mut q, edit).unwrap();
        assert!(q.adjusted);
    }
}

#[test]
fn cleanup_drops_unreferenced_sources() {
    let mut q = album();
    add(&mut q, source("m", 2), Anchor::ManualEnd);
    add(&mut q, source("n", 2), Anchor::ContextEnd);
    remove(&mut q, &["m.0"]);
    assert_eq!(q.sources.len(), 3);
    remove(&mut q, &["m.1"]);
    assert_eq!(q.sources.iter().map(|s| &*s.key).collect::<Vec<_>>(), ["a", "n"]);

    // Moving every row of a source keeps it.
    mv(&mut q, &["n.0", "n.1"], Anchor::ManualStart).unwrap();
    assert_eq!(q.sources.len(), 2);

    // Playing the rows out drops it, and the current item keeps its own source.
    next(&mut q);
    assert_eq!(current(&q), Some("n.0".into()));
    assert_eq!(q.sources.len(), 2);
    next(&mut q);
    next(&mut q);
    assert_eq!(current(&q), Some("a.1".into()));
    // History's plays keep what they name.
    assert_eq!(q.sources.iter().map(|s| &*s.key).collect::<Vec<_>>(), ["a", "n"]);
    let plays = [play(1, "n.0"), play(2, "n.1")];
    apply(&mut q, Edit::RemovePlays { plays: plays.to_vec() }).unwrap();
    assert_eq!(q.sources.iter().map(|s| &*s.key).collect::<Vec<_>>(), ["a"]);
}

#[test]
fn cleanup_keeps_what_copies_history_and_shuffle_name() {
    let span = |key: &str, start, len| Span::new(View::Key(key.into()), start, len);
    let mut q = album();
    q.sources.extend([source("s", 3), source("t", 3), source("u", 3), source("v", 3)]);
    q.copies.push(QueueCopy { key: "c".into(), of: SpanList::from_spans([span("s", 0, 2)]) });
    q.copies.push(QueueCopy { key: "d".into(), of: SpanList::from_spans([span("t", 0, 2)]) });
    q.manual = SpanList::from_spans([span("c", 0, 2)]);
    q.history.push(Run { first: 0, span: span("u", 0, 1) });
    q.shuffle = Some(Shuffle { seed: 1, base: SpanList::from_spans([span("v", 0, 3)]) });
    q.clean_up();
    let keys = |q: &QueueState| {
        let mut keys: Vec<_> =
            q.sources.iter().map(|s| &s.key).chain(q.copies.iter().map(|c| &c.key)).collect();
        keys.sort();
        keys.into_iter().cloned().collect::<Vec<_>>()
    };
    // `d` and its source `t` are unreferenced; `s` stays through `c`.
    assert_eq!(keys(&q), ["a", "c", "s", "u", "v"]);
}

#[test]
fn fragmentation_bound() {
    let n = 2_000;
    let mut q = QueueState::default();
    q.start(source("a", n), 0).unwrap();
    let mut rng = Rng(7);
    let mut removed = HashSet::new();
    for _ in 0..200 {
        let pos = 1 + rng.below(n - 1);
        if !removed.insert(pos) {
            continue;
        }
        let spans = span_count(&q);
        remove(&mut q, &[&format!("a.{pos}")]);
        assert!(span_count(&q) <= spans + 2, "a removal grew the spans by more than two");
    }
    for _ in 0..200 {
        let (pos, to) = (1 + rng.below(n - 1), 1 + rng.below(n - 1));
        let spans = span_count(&q);
        let _ = mv(&mut q, &[&format!("a.{pos}")], before(&format!("a.{to}")));
        // Taking a row out splits one span, and putting it in splits another and adds its own.
        assert!(span_count(&q) <= spans + 3, "a move grew the spans by more than three");
    }
    assert!(span_count(&q) <= 1 + 2 * 200 + 3 * 200);
}

#[test]
fn edits_serialize_as_tagged_json() {
    use serde_json::json;
    let edits = [
        (Edit::Add { source: source("b", 1), before: before("a.2") }, "add"),
        (Edit::Remove { items: items(&["a.1"]) }, "remove"),
        (Edit::Move { items: items(&["a.1"]), before: Anchor::ContextEnd }, "move"),
        (Edit::Jump { item: item("a.2") }, "jump"),
        (Edit::Next, "next"),
        (Edit::Repeat { mode: Repeat::All }, "repeat"),
        (Edit::Previous { key: "c".into() }, "previous"),
        (Edit::RemovePlays { plays: vec![play(3, "a.1")] }, "removePlays"),
        (Edit::Requeue { play: play(3, "a.1"), key: "c".into() }, "requeue"),
    ];
    for (edit, tag) in edits {
        let value = serde_json::to_value(&edit).unwrap();
        assert_eq!(value["type"], tag);
        assert_eq!(serde_json::from_value::<Edit>(value).unwrap(), edit);
    }
    assert_eq!(
        serde_json::to_value(Edit::Move { items: items(&["k.1"]), before: before("k.4") }).unwrap(),
        json!({"type": "move", "items": ["k.1"], "before": {"type": "before", "item": "k.4"}})
    );
    assert_eq!(
        serde_json::to_value(Edit::Add { source: source("b", 1), before: Anchor::ManualStart })
            .unwrap(),
        json!({"type": "add", "before": {"type": "manualStart"},
               "source": {"key": "b", "libraryId": "lib", "length": 1}})
    );
    assert_eq!(serde_json::to_value(Edit::Next).unwrap(), json!({"type": "next"}));
    assert_eq!(
        serde_json::to_value(Edit::Previous { key: "c".into() }).unwrap(),
        json!({"type": "previous", "key": "c"})
    );
    assert_eq!(
        serde_json::to_value(Edit::Rewind { play: play(3, "a.1"), key: "c".into() }).unwrap(),
        json!({"type": "rewind", "play": {"play": 3, "item": "a.1"}, "key": "c"})
    );
    assert_eq!(
        serde_json::to_value(Edit::RemovePlays { plays: vec![play(3, "a.1")] }).unwrap(),
        json!({"type": "removePlays", "plays": [{"play": 3, "item": "a.1"}]})
    );
    assert_eq!(
        serde_json::to_value(Edit::Requeue { play: play(3, "a.1"), key: "c".into() }).unwrap(),
        json!({"type": "requeue", "play": {"play": 3, "item": "a.1"}, "key": "c"})
    );
    assert_eq!(
        serde_json::to_value(Edit::Shuffle { on: true, seed: 7 }).unwrap(),
        json!({"type": "shuffle", "on": true, "seed": 7})
    );
}

/// A small deterministic generator (SplitMix64).
struct Rng(u64);

impl Rng {
    /// The next number below `n`.
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) % n
    }
}

/// What a shuffle holds, kept as plain lists: the frozen base in natural order, and the base
/// items whose shuffle row is still in the context. Only removals and moves out of the
/// context shrink the second.
#[derive(Debug, Clone, PartialEq)]
struct ModelShuffle {
    seed: u32,
    base: Vec<ItemId>,
    live: HashSet<ItemId>,
}

/// What Up Next, the passed context, the playing item and History should be, kept as plain
/// lists.
#[derive(Debug, PartialEq)]
struct Model {
    manual: Vec<ItemId>,
    context: Vec<ItemId>,
    /// The context rows before Up Next, the current context item last.
    passed: Vec<ItemId>,
    current: Option<ItemId>,
    /// Every play, oldest first, with its number.
    plays: Vec<(u64, ItemId)>,
    next_play: u64,
    repeat: Repeat,
    shuffle: Option<ModelShuffle>,
}

impl Model {
    /// Where `anchor` lands, as (is-manual, index), or `None` if it names an item that is gone
    /// or in `skip`.
    fn spot(&self, anchor: &Anchor, skip: &[ItemId]) -> Option<(bool, usize)> {
        Some(match anchor {
            Anchor::ManualStart => (true, 0),
            Anchor::ManualEnd => (true, self.manual.len()),
            Anchor::ContextEnd => (false, self.context.len()),
            Anchor::Before { item } if skip.contains(item) => return None,
            Anchor::Before { item } => match self.manual.iter().position(|i| i == item) {
                Some(i) => (true, i),
                None => (false, self.context.iter().position(|i| i == item)?),
            },
        })
    }

    /// Drops the passed place of each of `items` that is in the Manual Queue.
    fn drop_passed(&mut self, items: &[ItemId]) {
        let held: Vec<_> = self.manual.iter().filter(|i| items.contains(i)).cloned().collect();
        self.passed.retain(|i| !held.contains(i));
        self.forget(&held);
    }

    /// Takes `items` out of the shuffle's live rows.
    fn forget(&mut self, items: &[ItemId]) {
        if let Some(shuffle) = &mut self.shuffle {
            items.iter().for_each(|i| _ = shuffle.live.remove(i));
        }
    }

    /// Shuffles the context with `seed`: row `p` is the base row π gives it.
    fn shuffle_on(&mut self, seed: u32) {
        let base = std::mem::take(&mut self.context);
        let m = base.len() as u64;
        self.context = (0..m).map(|p| base[permute(seed, m, p) as usize].clone()).collect();
        self.shuffle = Some(ModelShuffle { seed, live: base.iter().cloned().collect(), base });
    }

    /// Unshuffles by the rules of `design/queue.md` §6, on plain lists.
    fn shuffle_off(&mut self) {
        let Some(ModelShuffle { base, live, .. }) = self.shuffle.take() else { return };
        let cursor = self.passed.len();
        let rows: Vec<ItemId> = self.passed.iter().chain(&self.context).cloned().collect();
        let first = rows.iter().position(|i| live.contains(i)).unwrap_or(rows.len());
        let base_row = |item: &ItemId| base.iter().position(|b| b == item);
        let latest = self.passed.iter().rev().find(|i| live.contains(*i));
        let pivot = self.current.as_ref().and_then(base_row).or_else(|| latest.and_then(base_row));

        // Passed rows after the first shuffle row that the base doesn't read are additions, and
        // so are Manual Queue tracks whose natural place is ahead of the cursor.
        let mut left_out = Vec::new();
        let mut passed: Vec<ItemId> = rows[..first.min(cursor)].to_vec();
        let mut extras = Vec::new();
        for item in &rows[first.min(cursor)..cursor] {
            let after = base_row(item).is_some_and(|b| pivot.is_none_or(|j| b > j));
            if !live.contains(item) || (self.manual.contains(item) && after) {
                extras.push(item.clone());
                if live.contains(item) {
                    left_out.push(item.clone());
                }
            }
        }
        let natural: Vec<_> =
            base.iter().filter(|i| live.contains(*i) && !left_out.contains(i)).collect();
        let split = pivot.map_or(0, |j| j + 1);
        let is_left = |i: &&ItemId| base_row(i).unwrap() < split;
        passed.extend(natural.iter().copied().filter(is_left).cloned());
        passed.extend(extras);
        // An addition ahead goes before the natural place of the base row that followed it, but
        // not into the passed part; a head row stays at the front; none following goes last.
        let right: Vec<ItemId> = natural.iter().copied().filter(|i| !is_left(i)).cloned().collect();
        let mut anchored: Vec<(usize, ItemId)> = Vec::new();
        let mut last = Vec::new();
        for (d, item) in self.context.iter().enumerate().filter(|(_, i)| !live.contains(*i)) {
            let following = self.context[d..].iter().find(|i| live.contains(*i)).and_then(base_row);
            if cursor + d < first {
                anchored.push((split, item.clone()));
            } else if let Some(b) = following {
                anchored.push((b.max(split), item.clone()));
            } else {
                last.push(item.clone());
            }
        }
        anchored.sort_by_key(|&(anchor, _)| anchor);
        let mut context = Vec::new();
        let mut waiting = anchored.into_iter().peekable();
        for item in right {
            let b = base_row(&item).unwrap();
            while let Some((_, added)) = waiting.next_if(|(anchor, _)| *anchor <= b) {
                context.push(added);
            }
            context.push(item);
        }
        context.extend(waiting.map(|(_, added)| added));
        context.extend(last);
        self.passed = passed;
        self.context = context;
    }

    /// Inserts `rows` at `spot`.
    fn put(&mut self, spot: (bool, usize), rows: Vec<ItemId>) {
        let list = if spot.0 { &mut self.manual } else { &mut self.context };
        list.splice(spot.1..spot.1, rows);
    }

    /// Records the item playing as a play.
    fn play_current(&mut self) {
        if let Some(item) = self.current.clone() {
            self.plays.push((self.next_play, item));
            self.next_play += 1;
        }
    }

    /// Applies `edit` the obvious way, and says whether it should fail.
    fn apply(&mut self, edit: &Edit) -> Result<(), QueueError> {
        match edit {
            Edit::Add { source, before } => {
                let spot = self.spot(before, &[]).ok_or(QueueError::AnchorGone)?;
                let rows =
                    (0..source.length).map(|pos| ItemId { key: source.key.clone(), pos }).collect();
                self.put(spot, rows);
            }
            Edit::Remove { items } => {
                self.drop_passed(items);
                let from_context: Vec<_> =
                    self.context.iter().filter(|i| items.contains(i)).cloned().collect();
                self.manual.retain(|i| !items.contains(i));
                self.context.retain(|i| !items.contains(i));
                self.forget(&from_context);
            }
            Edit::Move { items, before } => {
                let named: Vec<_> = items
                    .iter()
                    .filter(|i| self.manual.contains(i) || self.context.contains(i))
                    .cloned()
                    .collect();
                let target = self.spot(before, &named).ok_or(QueueError::AnchorGone)?;
                if !target.0 {
                    self.drop_passed(&named);
                }
                let moved: Vec<_> = self
                    .manual
                    .iter()
                    .chain(&self.context)
                    .filter(|i| named.contains(i))
                    .cloned()
                    .collect();
                let from_context: Vec<_> =
                    self.context.iter().filter(|i| named.contains(i)).cloned().collect();
                self.manual.retain(|i| !named.contains(i));
                self.context.retain(|i| !named.contains(i));
                if target.0 {
                    self.forget(&from_context);
                }
                let spot = self.spot(before, &[]).ok_or(QueueError::AnchorGone)?;
                self.put(spot, moved);
            }
            Edit::Jump { item } => {
                let skipped: Vec<ItemId> =
                    if let Some(at) = self.manual.iter().position(|i| i == item) {
                        self.manual.drain(..=at).collect()
                    } else if let Some(at) = self.context.iter().position(|i| i == item) {
                        let mut skipped: Vec<_> = std::mem::take(&mut self.manual);
                        let passed: Vec<_> = self.context.drain(..=at).collect();
                        skipped.extend(passed.iter().cloned());
                        self.passed.extend(passed);
                        skipped
                    } else {
                        return Ok(());
                    };
                self.play_current();
                for skipped in &skipped[..skipped.len() - 1] {
                    self.plays.push((self.next_play, skipped.clone()));
                    self.next_play += 1;
                }
                self.current = Some(item.clone());
            }
            Edit::Next => {
                self.play_current();
                if self.manual.is_empty() && self.context.is_empty() && self.repeat == Repeat::All {
                    self.context = std::mem::take(&mut self.passed);
                    // A shuffle with nothing to play shuffles the lap that starts.
                    match &self.shuffle {
                        Some(shuffle) if shuffle.base.is_empty() && !self.context.is_empty() => {
                            self.shuffle_on(shuffle.seed);
                        }
                        _ => {}
                    }
                }
                self.current = if !self.manual.is_empty() {
                    Some(self.manual.remove(0))
                } else if !self.context.is_empty() {
                    let item = self.context.remove(0);
                    self.passed.push(item.clone());
                    Some(item)
                } else {
                    None
                };
            }
            Edit::RemovePlays { plays } => {
                for p in plays {
                    self.plays.retain(|(no, item)| !(*no == p.play && *item == p.item));
                }
            }
            Edit::Requeue { play, key } => {
                let held = (play.play, play.item.clone());
                if let Some(at) = self.plays.iter().position(|p| *p == held) {
                    self.plays.remove(at);
                    self.manual.insert(0, ItemId { key: key.clone(), pos: 0 });
                }
            }
            Edit::Previous { key } => {
                let Some((_, last)) = self.plays.pop() else { return Ok(()) };
                if let Some(old) = self.current.take() {
                    if self.manual.is_empty() && self.passed.last() == Some(&old) {
                        self.passed.pop();
                        self.context.insert(0, old);
                    } else {
                        self.manual.insert(0, old);
                    }
                }
                let queued = self.manual.contains(&last) || self.context.contains(&last);
                self.current =
                    Some(if queued { ItemId { key: key.clone(), pos: 0 } } else { last });
            }
            Edit::Shuffle { on: true, seed } if self.shuffle.is_none() => self.shuffle_on(*seed),
            Edit::Shuffle { on: false, .. } => self.shuffle_off(),
            Edit::Shuffle { .. } => {}
            Edit::Rewind { .. } => unreachable!("random edits don't rewind"),
            Edit::Repeat { mode } => self.repeat = *mode,
        }
        Ok(())
    }
}

/// The queue's Up Next and playing item as a [`Model`].
fn model_of(state: &QueueState) -> Model {
    let row = |i| state.up_next_row(i).unwrap().item;
    Model {
        manual: (0..state.manual.len()).map(row).collect(),
        context: (state.manual.len()..state.up_next_len()).map(row).collect(),
        passed: order_items(state)[..state.cursor as usize].to_vec(),
        shuffle: state.shuffle.as_ref().map(|shuffle| ModelShuffle {
            seed: shuffle.seed,
            base: list_items(&shuffle.base),
            live: state
                .order
                .spans()
                .iter()
                .filter(|s| s.view == View::Shuffle)
                .flat_map(|s| s.start..s.start + s.len)
                .map(|p| state.item_of(&View::Shuffle, p).unwrap())
                .collect(),
        }),
        current: state.current.clone(),
        plays: plays(state),
        next_play: state.next_play,
        repeat: state.repeat,
    }
}

/// A random edit for `model`'s queue, mixing real and made-up items, anchors and plays. `keys`
/// counts the source keys minted so far (`s0` ... ) and grows when the edit mints one.
fn random_edit(rng: &mut Rng, model: &Model, keys: &mut u64) -> Edit {
    // Items to name: queued ones, gone ones and made-up ones.
    let queued: Vec<ItemId> = model.manual.iter().chain(&model.context).cloned().collect();
    let known = *keys;
    let pick = |rng: &mut Rng| {
        if !queued.is_empty() && rng.below(2) == 0 {
            return queued[rng.below(queued.len() as u64) as usize].clone();
        }
        let key = format!("s{}", rng.below(known + 1));
        ItemId { key, pos: rng.below(32) }
    };
    let count = rng.below(4) as usize;
    let named: Vec<ItemId> = (0..count).map(|_| pick(rng)).collect();
    let anchor = match rng.below(5) {
        0 => Anchor::ManualStart,
        1 => Anchor::ManualEnd,
        2 => Anchor::ContextEnd,
        _ => Anchor::Before { item: pick(rng) },
    };
    // A play to name: a real one, a real number with the wrong item, or a made-up one.
    let play_ref = |rng: &mut Rng| match model.plays.len() as u64 {
        0 => play(rng.below(3), "s0.0"),
        n => {
            let (no, item) = model.plays[rng.below(n) as usize].clone();
            match rng.below(4) {
                0 => PlayRef { play: no, item: pick(rng) },
                1 => PlayRef { play: no + 1, item },
                _ => PlayRef { play: no, item },
            }
        }
    };
    let mut mint = || {
        let key = format!("s{keys}");
        *keys += 1;
        key
    };
    match rng.below(20) {
        0..=2 => Edit::Add { source: source(&mint(), rng.below(6)), before: anchor },
        3 | 4 => Edit::Remove { items: named },
        5 | 6 => Edit::Move { items: named, before: anchor },
        7 => Edit::Jump { item: pick(rng) },
        8 | 9 => Edit::Next,
        10 => Edit::RemovePlays { plays: (0..count).map(|_| play_ref(rng)).collect() },
        11 => Edit::Requeue { play: play_ref(rng), key: mint() },
        12..=14 => Edit::Previous { key: mint() },
        15 | 16 => {
            Edit::Repeat { mode: [Repeat::Off, Repeat::All, Repeat::One][rng.below(3) as usize] }
        }
        _ => Edit::Shuffle { on: rng.below(2) == 0, seed: rng.below(1 << 32) as u32 },
    }
}

/// A queue over a random source `s0`, and the model that follows it.
fn random_queue(rng: &mut Rng) -> (QueueState, Model) {
    let mut q = QueueState::default();
    // Short queues run out, restart and shuffle their laps often.
    let longest = if rng.below(2) == 0 { 4 } else { 30 };
    let length = 1 + rng.below(longest);
    q.start(source("s0", length), rng.below(length)).unwrap();
    let model = model_of(&q);
    (q, model)
}

#[test]
fn random_edits_keep_invariants() {
    let (mut turned_on, mut turned_off, mut edited, mut shuffled_edits) = (0, 0, 0, 0);
    for seed in 0..60 {
        let mut rng = Rng(seed);
        let (mut q, mut model) = random_queue(&mut rng);
        let mut keys = 1;
        let mut changed = false;
        for _ in 0..150 {
            let edit = random_edit(&mut rng, &model, &mut keys);
            let was = q.clone();
            let expected = model.apply(&edit);
            let got = apply(&mut q, edit.clone());
            assert_eq!(got, expected, "seed {seed}: {edit:?}");
            if got.is_err() {
                assert_eq!(q, was);
            }
            assert_eq!(model_of(&q), model, "seed {seed}: {edit:?}");

            match edit {
                Edit::Shuffle { on: true, .. } if was.shuffle.is_none() => {
                    (turned_on, changed) = (turned_on + 1, false);
                }
                Edit::Shuffle { on: false, .. } if was.shuffle.is_some() => {
                    turned_off += 1;
                    edited += usize::from(changed);
                }
                Edit::Add { .. } | Edit::Remove { .. } | Edit::Move { .. } | Edit::Jump { .. } => {
                    changed |= q.shuffle.is_some() && q != was;
                }
                _ => {}
            }
            shuffled_edits += usize::from(was.shuffle.is_some() && q != was);
        }
    }
    assert!(turned_on > 150 && turned_off > 100, "{turned_on} shuffles, {turned_off} unshuffles");
    assert!(edited > 110 && shuffled_edits > 1_200, "{edited} edited, {shuffled_edits} edits");
}

#[test]
fn previous_restores_up_next() {
    let (mut stepped_back, mut shuffled_back) = (0, 0);
    for seed in 0..60 {
        let mut rng = Rng(1_000 + seed);
        let (mut q, mut model) = random_queue(&mut rng);
        let mut keys = 1;
        for _ in 0..150 {
            let edit = if rng.below(3) == 0 {
                keys += 1;
                Edit::Previous { key: format!("s{}", keys - 1) }
            } else {
                random_edit(&mut rng, &model, &mut keys)
            };
            let up_next = |q: &QueueState| -> Vec<String> {
                (0..q.up_next_len()).map(|i| q.up_next_row(i).unwrap().item.to_string()).collect()
            };
            let (was, before, had_plays) = (q.clone(), up_next(&q), q.history_len() > 0);
            let expected = model.apply(&edit);
            assert_eq!(apply(&mut q, edit.clone()), expected, "seed {seed}: {edit:?}");
            assert_eq!(model_of(&q), model, "seed {seed}: {edit:?}");
            if !matches!(edit, Edit::Previous { .. }) {
                continue;
            }
            if !had_plays {
                assert_eq!(q, was, "Previous with no History changed the queue");
                continue;
            }
            // Up Next is the old current, then Up Next as it was.
            stepped_back += 1;
            shuffled_back += usize::from(was.shuffle.is_some());
            let mut want: Vec<String> = was.current.iter().map(ToString::to_string).collect();
            want.extend(before);
            assert_eq!(up_next(&q), want, "seed {seed}: {edit:?}");
            assert_eq!(q.next_play, was.next_play);
            assert_eq!(q.history_len(), was.history_len() - 1);
            assert_eq!(q.adjusted, was.adjusted);
            assert!(q.copies.len() <= was.copies.len() + 1);
        }
    }
    assert!(stepped_back > 500, "only {stepped_back} Previous edits ran with plays");
    assert!(shuffled_back > 350, "only {shuffled_back} of them ran shuffled");
}

/// What a queue plays, by what each row reads: the playing row, Up Next with each Manual Queue
/// row's second, passed place, the passed context, the cursor, and what History keeps.
fn reads(state: &QueueState) -> impl PartialEq + std::fmt::Debug {
    let read = |row: Option<Row<'_>>| row.map(|r| (r.source.to_string(), r.position));
    let passed = order_items(state);
    let up_next: Vec<_> = (0..state.up_next_len())
        .map(|i| {
            let row = state.up_next_row(i).unwrap();
            let two_place =
                i < state.manual.len() && passed[..state.cursor as usize].contains(&row.item);
            (read(Some(row)), two_place)
        })
        .collect();
    let context: Vec<_> = (0..state.cursor).map(|i| read(state.context_row(i))).collect();
    (
        read(state.current_row()),
        up_next,
        state.manual.len(),
        context,
        state.cursor,
        plays(state),
        (state.next_play, state.adjusted, state.repeat),
    )
}

/// Rewinds a copy of `state` to the play numbered `no`, and checks it reads as Previous
/// repeated until that play is current, with at most one new copy. Returns the rewound queue.
fn rewound(state: &QueueState, no: u64, key: &str) -> QueueState {
    let (_, item) = plays(state).into_iter().find(|(n, _)| *n == no).unwrap();
    let mut by_previous = state.clone();
    for n in 0..plays(state).iter().filter(|(n, _)| *n >= no).count() {
        by_previous.apply(&Edit::Previous { key: format!("{key}-{n}") }).unwrap();
    }
    let mut q = state.clone();
    apply(&mut q, Edit::Rewind { play: PlayRef { play: no, item }, key: key.into() }).unwrap();
    assert_eq!(reads(&q), reads(&by_previous));
    assert!(q.copies.len() <= state.copies.len() + 1);
    q
}

/// A queue over `a` of `n` tracks on Repeat All, played until it restarted at `a.0`.
fn restarted(n: u64) -> QueueState {
    let mut q = QueueState::default();
    q.start(source("a", n), 0).unwrap();
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    for _ in 0..n {
        next(&mut q);
    }
    q
}

#[test]
fn rewind_replays_in_order_heard() {
    // Hear a.0, a.1, m.0 (queued), a.2, with a.3 playing: five tracks, the third queued.
    let mut q = album();
    next(&mut q);
    add(&mut q, source("m", 1), Anchor::ManualStart);
    next(&mut q);
    next(&mut q);
    next(&mut q);
    assert_eq!(current(&q), Some("a.3".into()));

    let mut q = rewound(&q, 0, "r");
    assert_eq!(current(&q), Some("a.0".into()));
    assert_eq!(manual(&q), ["a.1", "m.0"]);
    assert_eq!(upcoming(&q), ["a.2", "a.3", "a.4"]);
    // The cursor went back over a.3 and a.2, and nothing was copied.
    assert_eq!(q.cursor, 2);
    assert!(q.copies.is_empty() && q.history.is_empty());
    let mut heard = Vec::new();
    for _ in 0..5 {
        next(&mut q);
        heard.extend(current(&q));
    }
    assert_eq!(heard, ["a.1", "m.0", "a.2", "a.3", "a.4"]);
    // The numbers of the plays taken back aren't reused.
    assert_eq!(plays(&q)[0].0, 4);
}

#[test]
fn rewind_within_an_album_moves_only_the_cursor() {
    let mut q = album();
    for _ in 0..4 {
        next(&mut q);
    }
    let order = q.order.clone();
    // No copy is needed, so a key in use is fine.
    let q = rewound(&q, 1, "a");
    assert_eq!((current(&q), q.cursor, &q.order), (Some("a.1".into()), 2, &order));
    assert!(q.manual.is_empty() && q.copies.is_empty());
    assert_eq!(upcoming(&q), ["a.2", "a.3", "a.4"]);
    assert_eq!(runs(&q), [(0, "a".into(), 0, 1)]);
}

#[test]
fn rewind_keeps_waiting_manual_tracks_after() {
    let mut q = album();
    next(&mut q);
    add(&mut q, source("m", 1), Anchor::ManualStart);
    next(&mut q);
    next(&mut q);
    next(&mut q);
    add(&mut q, source("w", 2), Anchor::ManualEnd);
    assert_eq!((current(&q), manual(&q)), (Some("a.3".into()), vec!["w.0".into(), "w.1".into()]));

    let q = rewound(&q, 0, "r");
    assert_eq!(current(&q), Some("a.0".into()));
    // No phase 1 with the Manual Queue waiting: everything heard goes ahead of it.
    assert_eq!(q.cursor, 4);
    assert_eq!(manual(&q), ["a.1", "m.0", "a.2", "a.3", "w.0", "w.1"]);
    assert_eq!(upcoming(&q), ["a.4"]);
    assert!(q.copies.is_empty());
}

#[test]
fn rewind_over_mixed_run() {
    // a.1 is queued, so the run a.0, a.1, a.2 mixes a Manual play with context plays, and
    // `order` has no a.1 between a.0 and a.2.
    let mut q = album();
    mv(&mut q, &["a.1"], Anchor::ManualStart).unwrap();
    for _ in 0..3 {
        next(&mut q);
    }
    assert_eq!(current(&q), Some("a.3".into()));
    assert_eq!(runs(&q), [(0, "a".into(), 0, 3)]);

    let before = q;
    let q = rewound(&before, 0, "r");
    assert_eq!(current(&q), Some("a.0".into()));
    // Phase 1 takes a.3 and a.2; a.1 doesn't match the order, so it is a Manual piece.
    assert_eq!(q.cursor, 1);
    assert_eq!(
        (manual(&q), upcoming(&q)),
        (vec!["a.1".into()], ["a.2", "a.3", "a.4"].map(String::from).to_vec())
    );
    assert!(q.copies.is_empty() && q.history.is_empty());

    // Rewinding to the middle of the run splits it.
    let q = rewound(&before, 1, "r");
    assert_eq!(current(&q), Some("a.1".into()));
    assert_eq!(runs(&q), [(0, "a".into(), 0, 1)]);
}

#[test]
fn rewind_after_restart_copies() {
    let q = restarted(3);
    assert_eq!((current(&q), q.cursor), (Some("a.0".into()), 1));
    // Every play is in Up Next once the cursor goes back: one copy of all three.
    let mut by_rewind = rewound(&q, 0, "r");
    assert_eq!(by_rewind.copies.len(), 1);
    assert_eq!(by_rewind.copies[0].of.spans(), [Span::new(View::Key("a".into()), 0, 3)]);
    assert_eq!(current(&by_rewind), Some("r.0".into()));
    assert_eq!(manual(&by_rewind), ["r.1", "r.2"]);
    assert_eq!(upcoming(&by_rewind), ["a.0", "a.1", "a.2"]);
    assert_eq!(by_rewind.cursor, 0);
    // The copy plays the lap again, then the lap's tracks.
    let mut heard = Vec::new();
    for _ in 0..5 {
        next(&mut by_rewind);
        heard.extend(by_rewind.current_row().map(|r| r.position));
    }
    assert_eq!(heard, [1, 2, 0, 1, 2]);

    // A key in use fails the edit, changing nothing.
    let mut q = q;
    let fresh = q.clone();
    let rewind = |no, id, key: &str| Edit::Rewind { play: play(no, id), key: key.into() };
    assert_eq!(q.apply(&rewind(0, "a.0", "a")), Err(QueueError::DuplicateKey));
    assert_eq!(q, fresh);
    // Rewinding to the newest play with its item queued is Previous.
    let newest = rewound(&q, 2, "r");
    assert_eq!(current(&newest), Some("r.0".into()));
}

#[test]
fn rewind_after_split_run_one_copy() {
    let mut q = restarted(4);
    // Another device removes a.2, splitting the run.
    apply(&mut q, Edit::RemovePlays { plays: vec![play(2, "a.2")] }).unwrap();
    assert_eq!(runs(&q), [(0, "a".into(), 0, 2), (3, "a".into(), 3, 1)]);
    let before = q;
    let q = rewound(&before, 0, "r");
    assert_eq!(q.copies.len(), 1);
    assert_eq!(
        q.copies[0].of.spans(),
        [Span::new(View::Key("a".into()), 0, 2), Span::new(View::Key("a".into()), 3, 1)]
    );
    assert_eq!(current(&q), Some("r.0".into()));
    assert_eq!(manual(&q), ["r.1", "r.2"]);
    assert!(q.history.is_empty());

    // Rewinding to the piece after the split goes back over a single run.
    let q = rewound(&before, 3, "r");
    assert_eq!(runs(&q), [(0, "a".into(), 0, 2)]);
}

#[test]
fn rewind_flattens_copies() {
    // a.1 is re-queued as copy c, moved into the context and played on a later lap.
    let mut q = QueueState::default();
    q.start(source("a", 3), 0).unwrap();
    next(&mut q);
    next(&mut q);
    apply(&mut q, Edit::Requeue { play: play(1, "a.1"), key: "c".into() }).unwrap();
    mv(&mut q, &["c.0"], Anchor::ContextEnd).unwrap();
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    for _ in 0..3 {
        next(&mut q);
    }
    assert_eq!(current(&q), Some("a.1".into()));
    assert_eq!(plays(&q).last(), Some(&(4, item("a.0"))));

    // Play 3 is c.0, back in Up Next after the restart: the copy of it names a, not c.
    let q = rewound(&q, 3, "r");
    assert_eq!(
        q.copies.iter().find(|c| c.key == "r").unwrap().of.spans(),
        [Span::new(View::Key("a".into()), 1, 1)]
    );
    assert_eq!(current(&q), Some("r.0".into()));
    assert_eq!(q.current_row().unwrap().position, 1);
}

#[test]
fn rewind_stale_play_is_play_gone() {
    let mut q = album();
    for _ in 0..3 {
        next(&mut q);
    }
    let fresh = q.clone();
    let rewind = |no, id: &str| Edit::Rewind { play: play(no, id), key: "r".into() };
    // Wrong item, wrong number, a play past History, and a play another device removed.
    let used_key = Edit::Rewind { play: play(5, "a.1"), key: "a".into() };
    for edit in [rewind(1, "a.2"), rewind(5, "a.1"), rewind(3, "a.3"), rewind(1, "b.1"), used_key] {
        assert_eq!(apply(&mut q, edit), Err(QueueError::PlayGone));
        assert_eq!(q, fresh);
    }
    apply(&mut q, Edit::RemovePlays { plays: vec![play(1, "a.1")] }).unwrap();
    let removed = q.clone();
    assert_eq!(apply(&mut q, rewind(1, "a.1")), Err(QueueError::PlayGone));
    assert_eq!(q, removed);
    assert_eq!(QueueError::PlayGone.to_string(), "the play is no longer in History");
}

#[test]
fn rewind_equals_repeated_previous() {
    let (mut rewinds, mut with_copy, mut moved_cursor) = (0, 0, 0);
    let (mut shuffled, mut shuffled_copy, mut shuffled_cursor) = (0, 0, 0);
    for seed in 0..80 {
        let mut rng = Rng(5_000 + seed);
        let (mut q, mut model) = random_queue(&mut rng);
        let mut keys = 1;
        for step in 0..150 {
            let edit = random_edit(&mut rng, &model, &mut keys);
            let expected = model.apply(&edit);
            assert_eq!(apply(&mut q, edit.clone()), expected, "seed {seed}: {edit:?}");
            assert_eq!(model_of(&q), model, "seed {seed}: {edit:?}");
            let all = plays(&q);
            if all.is_empty() || rng.below(2) == 0 {
                continue;
            }
            let (no, _) = all[rng.below(all.len() as u64) as usize].clone();
            let by_rewind = rewound(&q, no, &format!("r{step}"));
            rewinds += 1;
            with_copy += usize::from(by_rewind.copies.len() > q.copies.len());
            moved_cursor += usize::from(by_rewind.cursor < q.cursor);
            if q.shuffle.is_some() {
                shuffled += 1;
                shuffled_copy += usize::from(by_rewind.copies.len() > q.copies.len());
                shuffled_cursor += usize::from(by_rewind.cursor < q.cursor);
            }
            if rng.below(3) == 0 {
                q = by_rewind;
                model = model_of(&q);
            }
        }
    }
    assert!(rewinds > 800, "only {rewinds} rewinds ran");
    assert!(with_copy > 15 && moved_cursor > 50, "{with_copy} copies, {moved_cursor} cursor moves");
    assert!(shuffled > 650, "only {shuffled} rewinds ran shuffled");
    assert!(
        shuffled_copy > 8 && shuffled_cursor > 90,
        "{shuffled_copy} copies, {shuffled_cursor} moves"
    );
}

#[test]
#[ignore = "timing run: cargo test -p jewelcase-core --release -- --ignored --nocapture"]
fn edits_at_500k_are_fast() {
    let bound =
        if cfg!(debug_assertions) { Duration::from_secs(1) } else { Duration::from_millis(1) };
    let mut q = QueueState::default();
    q.start(source("big", 500_000), 0).unwrap();
    add(&mut q, source("m", 3), Anchor::ManualEnd);

    let time = |what: &str, q: &mut QueueState, edit: Edit| {
        let started = Instant::now();
        q.apply(&edit).unwrap();
        let took = started.elapsed();
        println!("{what}: {took:?}");
        assert!(took < bound, "{what} took {took:?}");
    };
    time("add", &mut q, Edit::Add { source: source("n", 500_000), before: before("big.250000") });
    time(
        "move 2",
        &mut q,
        Edit::Move { items: items(&["big.400000", "big.300000"]), before: before("big.2") },
    );
    time("remove 2", &mut q, Edit::Remove { items: items(&["big.123456", "big.10"]) });
    time("next", &mut q, Edit::Next);
    time("jump into add", &mut q, Edit::Jump { item: item("n.499999") });
    assert_eq!(current(&q), Some("n.499999".into()));
    time("jump", &mut q, Edit::Jump { item: item("big.450000") });
    assert_eq!(current(&q), Some("big.450000".into()));
    assert_invariants(&q);
}

#[test]
#[ignore = "timing run: cargo test -p jewelcase-core --release -- --ignored --nocapture"]
fn removing_half_of_500k_scattered_items_is_fast() {
    let bound =
        if cfg!(debug_assertions) { Duration::from_secs(5) } else { Duration::from_millis(250) };
    let mut q = QueueState::default();
    q.start(source("big", 500_000), 0).unwrap();
    let every_other: Vec<ItemId> =
        (1..500_000).step_by(2).map(|pos| ItemId { key: "big".into(), pos }).collect();
    let started = Instant::now();
    q.apply(&Edit::Remove { items: every_other }).unwrap();
    let took = started.elapsed();
    println!("remove 250k scattered: {took:?}");
    assert!(took < bound, "took {took:?}");
    assert_eq!(q.up_next_len(), 249_999);
    assert_invariants(&q);
}

/// Splits a context of `5·k` rows into about `k` pieces, then moves `k` scattered rows to the
/// Manual Queue and removes them again. Returns how long the move and the removal took.
fn scattered_manual_edits(k: u64) -> (Duration, Duration) {
    let mut q = QueueState::default();
    q.start(source("big", 5 * k), 0).unwrap();
    let pick = |offset: u64| -> Vec<ItemId> {
        (0..k).map(|i| ItemId { key: "big".into(), pos: 5 * i + offset }).collect()
    };
    q.apply(&Edit::Remove { items: pick(4) }).unwrap();
    assert!(q.order.spans().len() as u64 >= k - 1);

    let started = Instant::now();
    q.apply(&Edit::Move { items: pick(2), before: Anchor::ManualEnd }).unwrap();
    let moved = started.elapsed();
    assert_eq!(q.manual.len(), k);

    let started = Instant::now();
    q.apply(&Edit::Remove { items: pick(2) }).unwrap();
    let removed = started.elapsed();
    assert!(q.manual.is_empty());
    assert_eq!(q.up_next_len(), 3 * k - 1);
    assert_invariants(&q);
    (moved, removed)
}

/// Finding each item an edit names is one sweep over each span list, not one scan per item,
/// which on 40k items over 40k pieces would take minutes.
#[test]
fn scattered_moves_to_the_manual_queue_and_back_out_are_fast() {
    let bound =
        if cfg!(debug_assertions) { Duration::from_secs(5) } else { Duration::from_secs(1) };
    let (moved, removed) = scattered_manual_edits(40_000);
    assert!(moved < bound, "move took {moved:?}");
    assert!(removed < bound, "remove took {removed:?}");
}

#[test]
#[ignore = "timing run: cargo test -p jewelcase-core --release -- --ignored --nocapture"]
fn scattered_manual_edits_at_370k_are_fast() {
    let (moved, removed) = scattered_manual_edits(370_000);
    println!("move 370k scattered to the Manual Queue: {moved:?}; remove them: {removed:?}");
    assert!(moved < Duration::from_secs(1) && removed < Duration::from_secs(1));
}

#[test]
#[ignore = "timing run: cargo test -p jewelcase-core --release -- --ignored --nocapture"]
fn rewind_across_1000_runs_is_fast() {
    let bound =
        if cfg!(debug_assertions) { Duration::from_secs(1) } else { Duration::from_millis(5) };
    // 1,000 runs alternating between two long lists; every `a` play is still in Up Next.
    let mut q = QueueState::default();
    q.start(source("a", 5_000), 0).unwrap();
    q.sources.push(source("b", 5_000));
    for i in 0..1_000 {
        let view = View::Key(if i % 2 == 0 { "a" } else { "b" }.into());
        q.history.push(Run { first: i, span: Span::new(view, i + 1, 1) });
    }
    q.next_play = 1_000;
    assert_invariants(&q);
    let target = PlayRef { play: 0, item: item("a.1") };
    let started = Instant::now();
    q.apply(&Edit::Rewind { play: target, key: "r".into() }).unwrap();
    let took = started.elapsed();
    println!("rewind across 1,000 runs: {took:?}");
    assert_invariants(&q);
    assert!(took < bound, "rewind took {took:?}");
    assert_eq!(q.manual.len(), 999);
}

/// Turns shuffle on with `seed`.
fn shuffle(state: &mut QueueState, seed: u32) {
    apply(state, Edit::Shuffle { on: true, seed }).unwrap();
}

/// Turns shuffle off.
fn unshuffle(state: &mut QueueState) {
    apply(state, Edit::Shuffle { on: false, seed: 0 }).unwrap();
}

/// The first seed from `0` whose shuffle of `state` satisfies `wanted`.
fn seed_where(state: &QueueState, wanted: impl Fn(&QueueState) -> bool) -> u32 {
    (0..200_000)
        .find(|&seed| {
            let mut q = state.clone();
            q.apply(&Edit::Shuffle { on: true, seed }).unwrap();
            wanted(&q)
        })
        .expect("no seed gives the wanted shuffle")
}

/// The context's Up Next, passed rows and the cursor as strings.
fn context(state: &QueueState) -> (Vec<String>, Vec<String>) {
    let items = order_items(state);
    let passed = items[..state.cursor as usize].iter().map(ToString::to_string).collect();
    (passed, upcoming(state))
}

#[test]
fn shuffled_reads_follow_the_permutation() {
    let mut q = album();
    next(&mut q);
    add(&mut q, source("b", 3), Anchor::ContextEnd);
    shuffle(&mut q, 77);
    // Up Next is every unplayed context row once, read as base[π(p)].
    let base: Vec<ItemId> = list_items(&q.shuffle.as_ref().unwrap().base);
    assert_eq!(base.len(), 6);
    assert_eq!(q.up_next_len(), 6);
    let mut seen = HashSet::new();
    for p in 0..6 {
        let row = q.up_next_row(p).unwrap();
        assert_eq!(row.item, base[permute(77, 6, p) as usize]);
        assert_eq!(q.find_item(&row.item), Some(p));
        assert!(seen.insert(row.item));
    }
    assert!(q.up_next_row(6).is_none());
    // The passed context and the playing item still read as before.
    assert_eq!((q.cursor, current(&q)), (2, Some("a.1".into())));
    assert_eq!(q.context_row(0).unwrap().item, item("a.0"));
    assert_eq!(q.context_row(2).unwrap().item, q.up_next_row(0).unwrap().item);
    // A base item that is gone from the shuffled rows isn't found.
    remove(&mut q, &["b.1"]);
    assert_eq!((q.find_item(&item("b.1")), q.up_next_len()), (None, 5));
    // Turning shuffle on again, or off when off, changes nothing.
    let shuffled = q.clone();
    shuffle(&mut q, 5);
    assert_eq!(q, shuffled);
    unshuffle(&mut q);
    let natural = q.clone();
    unshuffle(&mut q);
    assert_eq!(q, natural);
}

#[test]
fn shuffle_reads_duplicate_tracks_by_position() {
    let mut q = album();
    // Positions 1 and 3 hold the same track.
    q.attach_list("a", "h".into(), Arc::from([10, 11, 12, 11, 14])).unwrap();
    shuffle(&mut q, 3);
    let tracks: Vec<_> = (0..4).map(|i| q.up_next_row(i).unwrap()).collect();
    assert_eq!(tracks.iter().filter(|r| r.track == Some(11)).count(), 2);
    let (first, second) = (item("a.1"), item("a.3"));
    assert_ne!(q.find_item(&first), q.find_item(&second));
    remove(&mut q, &["a.3"]);
    assert_eq!(q.find_item(&second), None);
    assert!(q.find_item(&first).is_some());
    assert_eq!(q.up_next_len(), 3);
}

#[test]
fn unshuffle_without_edits_restores_spans() {
    // A queue with several spans, a Manual Queue and a mid-queue position.
    let mut q = QueueState::default();
    q.start(source("a", 10), 3).unwrap();
    add(&mut q, source("b", 4), Anchor::ContextEnd);
    add(&mut q, source("c", 2), before("a.6"));
    add(&mut q, source("m", 2), Anchor::ManualEnd);
    for seed in [0, 1, 99, u32::MAX] {
        let before = q.clone();
        shuffle(&mut q, seed);
        assert_eq!(q.order.spans().last().unwrap().view, View::Shuffle);
        assert_eq!(q.shuffle.as_ref().unwrap().base.len(), q.up_next_len() - q.manual.len());
        unshuffle(&mut q);
        assert_eq!(q, before, "seed {seed}");
        assert!(!q.adjusted || before.adjusted);
    }

    // Nothing upcoming: the shuffle holds no rows, and turning it off changes nothing.
    let mut q = album();
    for _ in 0..5 {
        next(&mut q);
    }
    let spent = q.clone();
    shuffle(&mut q, 4);
    assert!(q.shuffle.is_some() && q.order == spent.order);
    unshuffle(&mut q);
    assert_eq!(q, spent);
}

#[test]
fn unshuffle_resumes_natural_order() {
    // Play 1 of 10, shuffle, hear 7, 9 and then 3.
    let mut start = QueueState::default();
    start.start(source("a", 10), 0).unwrap();
    let seed = seed_where(&start, |q| {
        let first: Vec<_> = (0..3).map(|i| q.up_next_row(i).unwrap().item.to_string()).collect();
        first == ["a.6", "a.8", "a.2"]
    });
    let mut q = start.clone();
    shuffle(&mut q, seed);
    for _ in 0..3 {
        next(&mut q);
    }
    assert_eq!(current(&q), Some("a.2".into()));
    // Each shuffled play is a run of its own, as the list position it played.
    assert_eq!(runs(&q), [(0, "a".into(), 0, 1), (1, "a".into(), 6, 1), (2, "a".into(), 8, 1)]);

    unshuffle(&mut q);
    let want = |from: u64, to: u64| (from..to).map(|i| format!("a.{i}")).collect::<Vec<_>>();
    assert_eq!(context(&q), (want(0, 3), want(3, 10)));
    assert_eq!((current(&q), q.order.spans().len()), (Some("a.2".into()), 1));

    // A Repeat All lap plays 1 to 10 once each.
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    let mut lap = Vec::new();
    for _ in 0..10 {
        next(&mut q);
        lap.extend(current(&q));
    }
    assert_eq!(lap, [want(3, 10), want(0, 3)].concat());
}

#[test]
fn repeat_all_replays_the_shuffled_order() {
    let mut q = album();
    shuffle(&mut q, 11);
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    let order = upcoming(&q);
    assert_eq!(order.len(), 4);
    let mut heard = Vec::new();
    for _ in 0..9 {
        next(&mut q);
        heard.extend(current(&q));
    }
    // The shuffled order, then the next lap: what was passed before the shuffle, and the same
    // shuffled order again, with no copy and no new shuffle.
    assert_eq!(heard, [order.clone(), vec!["a.0".to_string()], order].concat());
    assert!(q.copies.is_empty());
    assert_eq!(q.shuffle.as_ref().unwrap().seed, 11);
}

/// The positions of the base `items` in natural order, as a string list for comparisons.
fn names(items: &[ItemId]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

#[test]
fn unshuffle_with_edits() {
    // `a` holds duplicate tracks: positions are items, not tracks.
    let mut q = QueueState::default();
    q.start(source("a", 12), 1).unwrap();
    q.attach_list("a", "h".into(), Arc::from([1, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3, 4])).unwrap();
    add(&mut q, source("b", 3), Anchor::ContextEnd);
    shuffle(&mut q, 2024);
    let shuffled: Vec<ItemId> =
        (0..q.up_next_len()).map(|i| q.up_next_row(i).unwrap().item).collect();
    assert_eq!(shuffled.len(), 13);
    let s = |i: usize| shuffled[i].to_string();

    // While shuffled: an addition ahead, two removals, and a move within the context.
    add(&mut q, source("c", 2), before(&s(3)));
    remove(&mut q, &[&s(0), &s(5)]);
    mv(&mut q, &[&s(9)], before(&s(1))).unwrap();
    assert_eq!(q.find_item(&item("c.0")), Some(2 + 1));
    next(&mut q);
    next(&mut q);
    assert_eq!(current(&q), Some(s(1)));
    assert_eq!(upcoming(&q)[..3], [s(2), "c.0".into(), "c.1".into()]);

    // A big jump to a track added last passes every row that was still ahead.
    add(&mut q, source("d", 1), Anchor::ContextEnd);
    jump(&mut q, "d.0");
    assert_eq!(q.up_next_len(), 0);
    let heard = order_items(&q);
    let last_base = heard.iter().rev().find(|i| i.key == "a" || i.key == "b").unwrap().clone();
    // Now more is added, ahead and in the Manual Queue.
    add(&mut q, source("e", 2), Anchor::ContextEnd);
    add(&mut q, source("m", 1), Anchor::ManualEnd);
    let was_adjusted = q.adjusted;
    unshuffle(&mut q);
    assert_eq!(q.adjusted, was_adjusted);
    assert!(q.shuffle.is_none());

    // Natural order less the removed rows. The pivot is the latest passed base item, since
    // the playing item is an addition.
    let natural: Vec<ItemId> = (2..12)
        .map(|p| format!("a.{p}"))
        .chain((0..3).map(|p| format!("b.{p}")))
        .map(|id| item(&id))
        .filter(|i| *i != shuffled[0] && *i != shuffled[5])
        .collect();
    let j = natural.iter().position(|i| *i == last_base).unwrap();
    let (passed, ahead) = context(&q);
    let want_passed = [
        names(&[item("a.0"), item("a.1")]),
        names(&natural[..=j]),
        names(&[item("c.0"), item("c.1"), item("d.0")]),
    ]
    .concat();
    assert_eq!(passed, want_passed);
    let want_ahead = [names(&natural[j + 1..]), names(&[item("e.0"), item("e.1")])].concat();
    assert_eq!(ahead, want_ahead);
    assert_eq!((current(&q), manual(&q)), (Some("d.0".into()), vec!["m.0".into()]));
    // The plays kept their list positions: the two heard, and what the jump passed.
    assert!(plays(&q).iter().all(|(_, i)| i.key == "a" || i.key == "b" || i.key == "c"));
    assert_eq!(q.history_len(), 1 + 1 + 1 + (heard.len() as u64 - 3 - 2));
}

#[test]
fn unshuffle_keeps_two_place_lap() {
    // Hear s0 and s1 of a shuffled album, queue m, and step back: s1 sits in the Manual Queue
    // and keeps its passed place, with s0 playing again.
    let build = |seed| {
        let mut q = album();
        shuffle(&mut q, seed);
        next(&mut q);
        next(&mut q);
        add(&mut q, source("m", 1), Anchor::ManualEnd);
        previous(&mut q, "p").unwrap();
        q
    };
    let position = |id: &str| id.rsplit('.').next().unwrap().parse::<u64>().unwrap();
    let seed_of = |after: bool| {
        (0..).find(|&seed| {
            let q = build(seed);
            let (pivot, held) = (position(&current(&q).unwrap()), position(&manual(&q)[0]));
            (held > pivot) == after
        })
    };

    // Its natural place is after the pivot: it stays in the Manual Queue, is left out of Up
    // Next, and keeps a passed place just before the cursor.
    let mut q = build(seed_of(true).unwrap());
    let (held, pivot) = (manual(&q)[0].clone(), current(&q).unwrap());
    assert_eq!(q.find_item(&item(&held)), Some(0));
    unshuffle(&mut q);
    let (passed, ahead) = context(&q);
    assert_eq!(manual(&q), [held.clone(), "m.0".into()]);
    assert!(!ahead.contains(&held));
    assert_eq!(passed.last(), Some(&held));
    assert_eq!(passed[passed.len() - 2], pivot);
    let at = position(&pivot);
    let rest: Vec<String> = (at + 1..5).map(|p| format!("a.{p}")).filter(|i| *i != held).collect();
    assert_eq!(ahead, rest);
    // The lap plays it: the Manual Queue, the rest of the context, then the whole context.
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    let mut heard = Vec::new();
    let lap_len = at as usize + 2;
    for _ in 0..2 + rest.len() + lap_len {
        next(&mut q);
        heard.extend(current(&q));
    }
    let lap = &heard[2 + rest.len()..];
    assert_eq!(lap.iter().filter(|i| **i == held).count(), 1);
    assert_eq!(lap.iter().collect::<HashSet<_>>().len(), lap_len);

    // Its natural place is before the pivot: it is passed there, and nothing is left out.
    let mut q = build(seed_of(false).unwrap());
    let (held, pivot) = (manual(&q)[0].clone(), current(&q).unwrap());
    unshuffle(&mut q);
    let (passed, ahead) = context(&q);
    let at = position(&pivot);
    let want: Vec<String> = (0..=at).map(|p| format!("a.{p}")).collect();
    assert_eq!(passed, want);
    assert!(passed.contains(&held) && !ahead.contains(&held));
    assert_eq!(manual(&q), [held, "m.0".into()]);
    assert_eq!(ahead.len(), 4 - at as usize);
}

#[test]
fn shuffled_plays_and_moves_record_list_positions() {
    // Moving shuffled rows into the Manual Queue puts in the list positions they play.
    let mut q = album();
    shuffle(&mut q, 8);
    let ahead = upcoming(&q);
    let ids: Vec<&str> = ahead[..3].iter().map(String::as_str).collect();
    mv(&mut q, &ids, Anchor::ManualEnd).unwrap();
    assert_eq!(manual(&q), ids);
    assert!(q.manual.spans().iter().all(|s| s.view != View::Shuffle));
    assert_eq!(upcoming(&q), ahead[3..]);
    // Moved back into the shuffled context it is an addition, ahead of the row it was put before.
    mv(&mut q, &[ids[1]], before(&ahead[3])).unwrap();
    assert_eq!(upcoming(&q)[0], ids[1]);
    assert_eq!(manual(&q), [ids[0], ids[2]]);

    // A jump over 2,500 shuffled rows records the newest 1,000 runs, and counts every play.
    let mut q = QueueState::default();
    q.start(source("a", 3_000), 0).unwrap();
    shuffle(&mut q, 31);
    let rows: Vec<ItemId> = (0..2_501).map(|i| q.up_next_row(i).unwrap().item).collect();
    jump(&mut q, &rows[2_500].to_string());
    assert_eq!(q.next_play, 2_501);
    assert!(q.history.len() <= HISTORY_RUNS && q.history.len() > HISTORY_RUNS - 10);
    // Play `n` heard row `n - 1` of the shuffled order.
    let newest = plays(&q);
    assert_eq!(newest.last().unwrap().0, 2_500);
    for (no, played) in &newest {
        assert_eq!(*played, rows[*no as usize - 1]);
    }
    assert!(q.sources.iter().any(|s| s.key == "a"));
}

#[test]
fn previous_and_rewind_under_shuffle() {
    let mut q = QueueState::default();
    q.start(source("a", 7), 0).unwrap();
    shuffle(&mut q, 5);
    let order = upcoming(&q);
    for _ in 0..4 {
        next(&mut q);
    }
    assert_eq!(current(&q), Some(order[3].clone()));

    // Previous moves the cursor back over the shuffle row: no copy, and Up Next as it was.
    let mut back = q.clone();
    previous(&mut back, "p").unwrap();
    assert_eq!(current(&back), Some(order[2].clone()));
    assert_eq!(upcoming(&back), order[3..]);
    assert_eq!((back.cursor, back.copies.len()), (q.cursor - 1, 0));

    // Rewinding to the first play heard is the same, three times over.
    let first = plays(&q)[1].0;
    let back = rewound(&q, first, "r");
    assert_eq!(current(&back), Some(order[0].clone()));
    assert_eq!(upcoming(&back), order[1..]);
    assert!(back.copies.is_empty() && back.manual.is_empty());
    assert_eq!(back.cursor, 2);

    // After a restart the plays are still ahead in the shuffled order, so they come back as one copy.
    let mut q = QueueState::default();
    q.start(source("a", 5), 0).unwrap();
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    for _ in 0..5 {
        next(&mut q);
    }
    shuffle(&mut q, 9);
    next(&mut q);
    let copied = rewound(&q, 1, "r");
    assert_eq!(copied.copies.len(), 1);
    assert_eq!(copied.manual.len(), 3);
}

#[test]
fn rewind_over_a_long_run_through_a_shuffle() {
    // One run of 3,000 plays that a restart put back in Up Next, now behind a shuffle.
    let mut q = QueueState::default();
    q.start(source("a", 3_000), 0).unwrap();
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    jump(&mut q, "a.2999");
    next(&mut q);
    assert_eq!((current(&q), q.history.len()), (Some("a.0".into()), 1));
    shuffle(&mut q, 17);
    next(&mut q);
    next(&mut q);
    let back = rewound(&q, 0, "r");
    assert_eq!(current(&back), Some("r.0".into()));
    assert_eq!(back.copies.len(), 1);
    assert!(back.history.is_empty());
}

#[test]
fn held_rows_agree_between_the_two_strategies() {
    let mut compared = 0;
    for seed in 0..40 {
        let mut rng = Rng(9_000 + seed);
        let (mut q, mut model) = random_queue(&mut rng);
        let mut keys = 1;
        for _ in 0..150 {
            let edit = random_edit(&mut rng, &model, &mut keys);
            let _ = model.apply(&edit);
            let _ = q.apply(&edit);
            if q.shuffle.is_none() {
                continue;
            }
            let ranges: Vec<(String, u64, u64)> =
                q.sources.iter().map(|s| (s.key.clone(), 0, s.length)).collect();
            let from = rng.below(q.order.len() + 1);
            let by = |lookup| q.shuffle_held_by(from, &ranges, Some(lookup));
            assert_eq!(by(true), by(false), "seed {seed}, from {from}");
            assert_eq!(by(true), q.shuffle_held(from, &ranges));
            compared += 1;
        }
    }
    assert!(compared > 1_000, "only {compared} comparisons");
}

#[test]
fn shuffle_loop_bounded() {
    let mut q = QueueState::default();
    q.start(source("a", 400), 0).unwrap();
    // Each unshuffle resumes at the playing item's natural place, so the context runs out
    // quickly without Repeat All.
    q.apply(&Edit::Repeat { mode: Repeat::All }).unwrap();
    for round in 0..300 {
        q.apply(&Edit::Shuffle { on: true, seed: round }).unwrap();
        for _ in 0..7 {
            q.apply(&Edit::Next).unwrap();
        }
        assert!(q.shuffle.as_ref().unwrap().base.spans().len() <= 1);
        q.apply(&Edit::Shuffle { on: false, seed: 0 }).unwrap();
        assert!(q.shuffle.is_none() && q.copies.is_empty());
        assert!(q.history.len() <= HISTORY_RUNS, "{} runs", q.history.len());
        assert!(span_count(&q) <= 2, "{} spans", span_count(&q));
        assert_eq!(q.up_next_len(), 400 - q.cursor);
        if round % 50 == 0 {
            assert_invariants(&q);
        }
    }
    assert!(q.history.len() > 900, "only {} runs", q.history.len());
    assert_eq!(q.next_play, 300 * 7);
}

#[test]
#[ignore = "timing run: cargo test -p jewelcase-core --release -- --ignored --nocapture"]
fn unshuffle_at_500k_is_fast() {
    let bound =
        if cfg!(debug_assertions) { Duration::from_secs(1) } else { Duration::from_millis(1) };
    let mut q = QueueState::default();
    q.start(source("big", 500_000), 0).unwrap();
    add(&mut q, source("tail", 3), Anchor::ContextEnd);
    shuffle(&mut q, 4);
    let rows: Vec<String> =
        (0..9).map(|i| q.up_next_row(i * 3).unwrap().item.to_string()).collect();
    // Edits made while shuffled: removals, a move out, an addition and a long jump.
    remove(&mut q, &[&rows[0], &rows[1]]);
    mv(&mut q, &[&rows[2]], Anchor::ManualEnd).unwrap();
    add(&mut q, source("n", 2), before(&rows[3]));
    next(&mut q);
    let target = q.up_next_row(250_000).unwrap().item.to_string();
    let started = Instant::now();
    q.apply(&Edit::Jump { item: item(&target) }).unwrap();
    let jumped = started.elapsed();
    println!("jump over 250,000 shuffled rows: {jumped:?}");
    assert!(jumped < 20 * bound, "the jump took {jumped:?}");
    // Rewinding to the oldest play History kept goes back over its 1,000 shuffled runs.
    let (oldest, played) = plays(&q).into_iter().next().unwrap();
    let mut back = q.clone();
    let started = Instant::now();
    let edit = Edit::Rewind { play: PlayRef { play: oldest, item: played }, key: "r".into() };
    back.apply(&edit).unwrap();
    let rewound = started.elapsed();
    println!("rewind over 1,000 shuffled runs: {rewound:?}");
    assert!(rewound < 20 * bound, "the rewind took {rewound:?}");
    let started = Instant::now();
    q.apply(&Edit::Shuffle { on: false, seed: 0 }).unwrap();
    let took = started.elapsed();
    println!("unshuffle at 500k: {took:?}");
    assert_invariants(&q);
    assert!(took < bound, "unshuffle took {took:?}");
    assert!(q.shuffle.is_none() && span_count(&q) < 20);
}

#[test]
#[ignore = "timing run: cargo test -p jewelcase-core --release -- --ignored --nocapture"]
fn rewind_over_a_long_run_through_a_shuffle_is_fast() {
    let bound =
        if cfg!(debug_assertions) { Duration::from_secs(1) } else { Duration::from_millis(1) };
    // One run of 500,000 plays that a restart put back in Up Next, now behind a shuffle.
    let mut q = QueueState::default();
    q.start(source("big", 500_000), 0).unwrap();
    q.apply(&Edit::Repeat { mode: Repeat::All }).unwrap();
    q.apply(&Edit::Jump { item: item("big.499999") }).unwrap();
    q.apply(&Edit::Next).unwrap();
    q.apply(&Edit::Shuffle { on: true, seed: 6 }).unwrap();
    q.apply(&Edit::Next).unwrap();
    q.apply(&Edit::Next).unwrap();
    let started = Instant::now();
    q.apply(&Edit::Rewind { play: play(0, "big.0"), key: "r".into() }).unwrap();
    let took = started.elapsed();
    println!("rewind over a 500,000-play run behind a shuffle: {took:?}");
    assert!(took < bound, "rewind took {took:?}");
    assert_eq!(current(&q), Some("r.0".into()));
}

#[test]
fn shuffled_laps_match_the_model() {
    // Short queues on Repeat All, mostly played and shuffled, so laps restart often.
    let (mut laps, mut turned_off_before) = (0, 0);
    for seed in 0..80 {
        let mut rng = Rng(20_000 + seed);
        let (mut q, mut model) = random_queue(&mut rng);
        let mut keys = 1;
        let repeat = Edit::Repeat { mode: Repeat::All };
        model.apply(&repeat).unwrap();
        apply(&mut q, repeat).unwrap();
        for _ in 0..150 {
            let edit = match rng.below(5) {
                0..=1 => Edit::Next,
                2 => Edit::Shuffle { on: rng.below(3) != 0, seed: rng.below(1 << 32) as u32 },
                _ => random_edit(&mut rng, &model, &mut keys),
            };
            let was = q.clone();
            let expected = model.apply(&edit);
            assert_eq!(apply(&mut q, edit.clone()), expected, "seed {seed}: {edit:?}");
            assert_eq!(model_of(&q), model, "seed {seed}: {edit:?}");
            let spent = was.shuffle.as_ref().is_some_and(|s| s.base.is_empty());
            let restarts = was.manual.is_empty() && was.cursor >= was.order.len();
            let restarted = matches!(edit, Edit::Next) && was.repeat == Repeat::All && restarts;
            laps += usize::from(restarted && spent && !was.order.is_empty());
            turned_off_before += usize::from(
                matches!(edit, Edit::Shuffle { on: false, .. }) && spent && was.order == q.order,
            );
        }
    }
    assert!(laps > 45 && turned_off_before > 130, "{laps} shuffled laps, {turned_off_before} off");
}

#[test]
fn unshuffle_puts_additions_before_the_row_that_followed() {
    let start = || {
        let mut q = QueueState::default();
        q.start(source("a", 10), 0).unwrap();
        q
    };
    let natural = |from: u64, skip: &[&str]| -> Vec<String> {
        (from..10).map(|p| format!("a.{p}")).filter(|i| !skip.contains(&i.as_str())).collect()
    };

    // An album dropped at the end of a shuffled context is last again.
    let mut q = start();
    shuffle(&mut q, 3);
    add(&mut q, source("b", 2), Anchor::ContextEnd);
    unshuffle(&mut q);
    assert_eq!(upcoming(&q), [natural(1, &[]), vec!["b.0".into(), "b.1".into()]].concat());

    // One put before a shuffled row goes just before that row's natural place, together.
    let mut q = start();
    shuffle(&mut q, 3);
    let shuffled = upcoming(&q);
    add(&mut q, source("c", 2), before(&shuffled[3]));
    let at = natural(1, &[]).iter().position(|i| *i == shuffled[3]).unwrap();
    let mut want = natural(1, &[]);
    want.splice(at..at, ["c.0".to_string(), "c.1".to_string()]);
    let mut with_removed = q.clone();
    unshuffle(&mut q);
    assert_eq!(upcoming(&q), want);

    // If that row was removed, it is the next row that survived.
    remove(&mut with_removed, &[&shuffled[3]]);
    unshuffle(&mut with_removed);
    let after = natural(1, &[&shuffled[3]]).iter().position(|i| *i == shuffled[4]).unwrap();
    let mut want = natural(1, &[&shuffled[3]]);
    want.splice(after..after, ["c.0".to_string(), "c.1".to_string()]);
    assert_eq!(upcoming(&with_removed), want);

    // A row whose followers are before the pivot goes to the front of Up Next.
    let build = |seed| {
        let mut q = start();
        shuffle(&mut q, seed);
        let rows = upcoming(&q);
        add(&mut q, source("c", 1), before(&rows[1]));
        next(&mut q);
        (q, rows)
    };
    let position = |id: &str| id.rsplit('.').next().unwrap().parse::<u64>().unwrap();
    let seed = (0..).find(|&s| {
        let (_, rows) = build(s);
        position(&rows[1]) < position(&rows[0])
    });
    let (mut q, rows) = build(seed.unwrap());
    unshuffle(&mut q);
    let pivot = position(&rows[0]);
    assert_eq!(upcoming(&q), [vec!["c.0".to_string()], natural(pivot + 1, &[])].concat());
    assert_eq!(q.cursor, pivot + 1);
}

#[test]
fn shuffle_at_the_end_shuffles_the_next_lap() {
    // Repeat All, a five-track album played to its last track, then shuffle on.
    let played_out = || {
        let mut q = album();
        apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
        for _ in 0..4 {
            next(&mut q);
        }
        assert_eq!((current(&q), q.up_next_len()), (Some("a.4".into()), 0));
        q
    };
    let mut q = played_out();
    let before = q.clone();
    shuffle(&mut q, 21);
    // Nothing changes yet: the playing item is passed, and the lap is as it was.
    assert_eq!((&q.order, q.cursor, &q.current), (&before.order, before.cursor, &before.current));
    assert!(q.shuffle.as_ref().is_some_and(|s| s.base.is_empty()));
    let spent = q.clone();
    unshuffle(&mut q);
    assert_eq!(q, before);

    // The restart plays the whole context through the shuffle, each track once.
    let mut q = spent.clone();
    let mut lap = Vec::new();
    for _ in 0..5 {
        next(&mut q);
        lap.extend(current(&q));
    }
    let natural: Vec<ItemId> = (0..5).map(|p| item(&format!("a.{p}"))).collect();
    let want: Vec<String> =
        (0..5).map(|p| natural[permute(21, 5, p) as usize].to_string()).collect();
    assert_eq!(lap, want);
    assert_eq!(q.shuffle.as_ref().unwrap().base.len(), 5);
    assert!(q.copies.is_empty());
    // Another lap replays the same order.
    for want in &lap {
        next(&mut q);
        assert_eq!(current(&q), Some(want.clone()));
    }

    // Previous before the restart steps back as usual, and the restart still shuffles.
    let mut q = spent.clone();
    previous(&mut q, "p").unwrap();
    assert_eq!((current(&q), upcoming(&q)), (Some("a.3".into()), vec!["a.4".into()]));
    next(&mut q);
    assert!(q.shuffle.as_ref().unwrap().base.is_empty());
    next(&mut q);
    assert_eq!(q.shuffle.as_ref().unwrap().base.len(), 5);
    assert_eq!(current(&q), Some(want[0].clone()));
    // Stepping back over the restart finds the old lap's last track still queued: a copy plays.
    previous(&mut q, "q").unwrap();
    assert_eq!(current(&q), Some("q.0".into()));
    assert_eq!(q.up_next_len(), 5);

    // Unshuffling after the restart resumes the natural lap from the playing item.
    let mut q = spent.clone();
    next(&mut q);
    unshuffle(&mut q);
    let at = natural.iter().position(|i| Some(i.to_string()) == current(&q)).unwrap();
    let (passed, ahead) = context(&q);
    assert_eq!((passed.len(), ahead.len()), (at + 1, 4 - at));

    // Rows added before the restart are in the lap; turning shuffle on with Repeat All still off
    // shuffles the lap once Repeat All is on.
    let mut q = album();
    for _ in 0..4 {
        next(&mut q);
    }
    shuffle(&mut q, 5);
    add(&mut q, source("b", 2), Anchor::ContextEnd);
    apply(&mut q, Edit::Repeat { mode: Repeat::All }).unwrap();
    for _ in 0..3 {
        next(&mut q);
    }
    assert_eq!(current(&q), Some("b.1".into()));
    next(&mut q);
    assert_eq!(q.shuffle.as_ref().unwrap().base.len(), 7);
    let lap = order_items(&q);
    assert_eq!(lap.iter().collect::<HashSet<_>>().len(), 7);
    assert!(lap.contains(&item("b.0")) && lap.contains(&item("a.0")));
}

/// The cases the web build's `core-wasm.spec.ts` replays in WebAssembly, so native and wasm are
/// held to the same π vectors and the same edit sequence.
fn parity() -> serde_json::Value {
    serde_json::from_str(include_str!("parity.json")).unwrap()
}

#[test]
fn parity_edit_sequence() {
    let fixture = parity();
    let mut state: QueueState = serde_json::from_value(fixture["initial"].clone()).unwrap();
    for edit in fixture["edits"].as_array().unwrap() {
        apply(&mut state, serde_json::from_value(edit.clone()).unwrap()).unwrap();
    }
    assert_eq!(serde_json::to_value(&state).unwrap(), fixture["final"]);
}

#[test]
fn holds_item_covers_playing_queued_and_passed_items() {
    let mut q = album();
    next(&mut q);
    add(&mut q, source("b", 2), Anchor::ManualEnd);
    for held in ["a.0", "a.1", "a.4", "b.1"] {
        assert!(q.holds_item(&item(held)), "{held}");
    }
    remove(&mut q, &["b.1"]);
    for gone in ["b.1", "b.2", "a.5", "c.0"] {
        assert!(!q.holds_item(&item(gone)), "{gone}");
    }
}
