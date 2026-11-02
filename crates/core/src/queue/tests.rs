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
    // The Manual Queue's source went with its rows.
    assert_eq!(q.sources.len(), 1);
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

/// What Up Next and the playing item should be, kept as plain lists.
#[derive(Debug, PartialEq)]
struct Model {
    manual: Vec<ItemId>,
    context: Vec<ItemId>,
    current: Option<ItemId>,
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

    /// Inserts `rows` at `spot`.
    fn put(&mut self, spot: (bool, usize), rows: Vec<ItemId>) {
        let list = if spot.0 { &mut self.manual } else { &mut self.context };
        list.splice(spot.1..spot.1, rows);
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
                self.manual.retain(|i| !items.contains(i));
                self.context.retain(|i| !items.contains(i));
            }
            Edit::Move { items, before } => {
                let named: Vec<_> = items
                    .iter()
                    .filter(|i| self.manual.contains(i) || self.context.contains(i))
                    .cloned()
                    .collect();
                self.spot(before, &named).ok_or(QueueError::AnchorGone)?;
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
                if let Some(at) = self.manual.iter().position(|i| i == item) {
                    self.manual.drain(..=at);
                } else if let Some(at) = self.context.iter().position(|i| i == item) {
                    self.manual.clear();
                    self.context.drain(..=at);
                } else {
                    return Ok(());
                }
                self.current = Some(item.clone());
            }
            Edit::Next => {
                self.current = if !self.manual.is_empty() {
                    Some(self.manual.remove(0))
                } else if !self.context.is_empty() {
                    Some(self.context.remove(0))
                } else {
                    None
                };
            }
            Edit::Repeat { .. } => {}
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
        current: state.current.clone(),
    }
}

#[test]
fn random_edits_keep_invariants() {
    for seed in 0..60 {
        let mut rng = Rng(seed);
        let mut q = QueueState::default();
        let length = 1 + rng.below(30);
        q.start(source("s0", length), rng.below(length)).unwrap();
        let mut model = model_of(&q);
        let mut keys = 1;
        for _ in 0..150 {
            // Items to name: queued ones, gone ones and made-up ones.
            let queued: Vec<ItemId> = model.manual.iter().chain(&model.context).cloned().collect();
            let pick = |rng: &mut Rng| {
                if !queued.is_empty() && rng.below(2) == 0 {
                    return queued[rng.below(queued.len() as u64) as usize].clone();
                }
                let key = format!("s{}", rng.below(keys + 1));
                ItemId { key, pos: rng.below(32) }
            };
            let count = rng.below(4) as usize;
            let named: Vec<ItemId> = (0..count).map(|_| pick(&mut rng)).collect();
            let anchor = match rng.below(5) {
                0 => Anchor::ManualStart,
                1 => Anchor::ManualEnd,
                2 => Anchor::ContextEnd,
                _ => Anchor::Before { item: pick(&mut rng) },
            };
            let edit = match rng.below(10) {
                0..=2 => {
                    let key = format!("s{keys}");
                    keys += 1;
                    Edit::Add { source: source(&key, rng.below(6)), before: anchor }
                }
                3 | 4 => Edit::Remove { items: named },
                5 | 6 => Edit::Move { items: named, before: anchor },
                7 => Edit::Jump { item: pick(&mut rng) },
                8 => Edit::Next,
                _ => Edit::Repeat { mode: Repeat::All },
            };
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
