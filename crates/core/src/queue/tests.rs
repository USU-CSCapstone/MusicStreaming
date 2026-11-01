//! Tests of Up Next edits and clean-up, and the invariant checker every queue test can use.

use std::collections::HashSet;
use std::time::{Duration, Instant};

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

/// Checks what must hold after every edit: keys unique, every span reads a real row of a real
/// view, each item is in Up Next at most once and in `order` at most once, the playing item
/// isn't in Up Next, and no source or copy is unreferenced.
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
    check(&state.order, "order");
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
    let passed = list_items(&state.order);
    let unique: HashSet<_> = passed.iter().collect();
    assert_eq!(unique.len(), passed.len(), "an item is in order twice");
    let mut up_next = list_items(&state.manual);
    up_next.extend(list_items(&state.order).split_off(state.cursor as usize));
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
        serde_json::to_value(Edit::RemovePlays { plays: vec![play(3, "a.1")] }).unwrap(),
        json!({"type": "removePlays", "plays": [{"play": 3, "item": "a.1"}]})
    );
    assert_eq!(
        serde_json::to_value(Edit::Requeue { play: play(3, "a.1"), key: "c".into() }).unwrap(),
        json!({"type": "requeue", "play": {"play": 3, "item": "a.1"}, "key": "c"})
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
                self.manual.retain(|i| !items.contains(i));
                self.context.retain(|i| !items.contains(i));
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
                self.manual.retain(|i| !named.contains(i));
                self.context.retain(|i| !named.contains(i));
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
        passed: list_items(&state.order)[..state.cursor as usize].to_vec(),
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
    match rng.below(17) {
        0..=2 => Edit::Add { source: source(&mint(), rng.below(6)), before: anchor },
        3 | 4 => Edit::Remove { items: named },
        5 | 6 => Edit::Move { items: named, before: anchor },
        7 => Edit::Jump { item: pick(rng) },
        8 | 9 => Edit::Next,
        10 => Edit::RemovePlays { plays: (0..count).map(|_| play_ref(rng)).collect() },
        11 => Edit::Requeue { play: play_ref(rng), key: mint() },
        12..=14 => Edit::Previous { key: mint() },
        _ => Edit::Repeat { mode: [Repeat::Off, Repeat::All, Repeat::One][rng.below(3) as usize] },
    }
}

/// A queue over a random source `s0`, and the model that follows it.
fn random_queue(rng: &mut Rng) -> (QueueState, Model) {
    let mut q = QueueState::default();
    let length = 1 + rng.below(30);
    q.start(source("s0", length), rng.below(length)).unwrap();
    let model = model_of(&q);
    (q, model)
}

#[test]
fn random_edits_keep_invariants() {
    for seed in 0..60 {
        let mut rng = Rng(seed);
        let (mut q, mut model) = random_queue(&mut rng);
        let mut keys = 1;
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
        }
    }
}

#[test]
fn previous_restores_up_next() {
    let mut stepped_back = 0;
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
