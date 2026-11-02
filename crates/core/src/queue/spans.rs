//! Recipes, views and span lists (`design/queue.md` §2, §4).

use std::ops::Range;

use serde::{Deserialize, Serialize};

/// How a list was made. It isn't what plays (the hash is); it lets a device rebuild the list
/// and lets the interface say what is playing. Every field that changes the order is part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Recipe {
    /// An album's tracks in a sort.
    Album { album_id: String, sort: String },
    /// An artist's tracks, by scope and credit, in a sort.
    Artist { artist_id: String, scope: String, artist_credit: String, sort: String },
    /// A library's tracks under filters (name, value pairs), in a sort.
    Library { filters: Vec<(String, String)>, sort: String },
    /// A search query.
    Search { q: String },
    /// Explicit track IDs the device sent.
    Tracks,
}

/// What a span reads: a source's or a copy's key, or the queue's one shuffle.
///
/// Naming views by key means dropping a view renumbers nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum View {
    /// A source or a copy, by its device-minted key.
    Key(String),
    /// The shuffle of the context.
    Shuffle,
}

/// Positions `start..start + len` of a view. Never empty inside a [`SpanList`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// The view read.
    pub view: View,
    /// The first position.
    pub start: u64,
    /// How many positions, at least 1.
    pub len: u64,
}

impl Span {
    /// A span of `len` positions of `view` from `start`.
    pub fn new(view: View, start: u64, len: u64) -> Self {
        Self { view, start, len }
    }
}

/// An ordered run of spans with running lengths, so the span holding a row index is found in
/// O(log s) and every edit costs O(s) in the number of spans, never in rows.
///
/// Invariants: no span is empty, no two neighbours could be one span (same view, the second
/// starting where the first ends), and `ends[i]` is the rows through span `i`.
/// Lengths are `u64` and assumed not to overflow it together.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpanList {
    spans: Vec<Span>,
    ends: Vec<u64>,
}

impl SpanList {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// A list of `spans`, with empty ones dropped and adjacent ones merged.
    pub fn from_spans(spans: impl IntoIterator<Item = Span>) -> Self {
        let mut list = Self { spans: spans.into_iter().collect(), ends: Vec::new() };
        list.normalize();
        list
    }

    /// The spans, in order.
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// The total rows.
    pub fn len(&self) -> u64 {
        self.ends.last().copied().unwrap_or(0)
    }

    /// Whether there are no rows.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The span holding row `index` and the row's offset in it, or `None` past the end.
    pub fn locate(&self, index: u64) -> Option<(usize, u64)> {
        let i = self.ends.partition_point(|&end| end <= index);
        (i < self.spans.len()).then(|| (i, index - self.start_of(i)))
    }

    /// The view and position that row `index` reads, or `None` past the end.
    pub fn get(&self, index: u64) -> Option<(&View, u64)> {
        let (i, offset) = self.locate(index)?;
        let span = &self.spans[i];
        Some((&span.view, span.start + offset))
    }

    /// The index of the first row that reads position `pos` of `view`, or `None`. O(spans).
    pub fn find(&self, view: &View, pos: u64) -> Option<u64> {
        let i = self
            .spans
            .iter()
            .position(|s| s.view == *view && (s.start..s.start + s.len).contains(&pos))?;
        Some(self.start_of(i) + pos - self.spans[i].start)
    }

    /// The first row that reads each of `items` (sorted by key, then position), or `None` for
    /// an item no row reads. One pass over the spans with a binary search in `items` for each,
    /// so finding k items is O(s log k + k) rather than O(s) per item.
    pub fn find_sorted(&self, items: &[(&str, u64)]) -> Vec<Option<u64>> {
        let mut rows = vec![None; items.len()];
        for (i, span) in self.spans.iter().enumerate() {
            let View::Key(key) = &span.view else { continue };
            let (key, end) = (key.as_str(), span.start + span.len);
            let from = items.partition_point(|&item| item < (key, span.start));
            for (row, &(k, pos)) in rows[from..].iter_mut().zip(&items[from..]) {
                if k != key || pos >= end {
                    break;
                }
                row.get_or_insert(self.start_of(i) + pos - span.start);
            }
        }
        rows
    }

    /// Makes row `index` the start of a span, splitting the span that holds it, and returns
    /// that span's index (the span count when `index` is the end). An `index` past the end is
    /// the end.
    fn split_at(&mut self, index: u64) -> usize {
        let Some((i, offset)) = self.locate(index) else {
            return self.spans.len();
        };
        if offset == 0 {
            return i;
        }
        let tail = Span {
            view: self.spans[i].view.clone(),
            start: self.spans[i].start + offset,
            len: self.spans[i].len - offset,
        };
        self.spans[i].len = offset;
        self.spans.insert(i + 1, tail);
        self.ends.insert(i, index);
        i + 1
    }

    /// Inserts `spans` before row `index` (the end when past it). Empty spans are dropped and
    /// neighbours of one view merge.
    pub fn splice(&mut self, index: u64, spans: impl IntoIterator<Item = Span>) {
        let at = self.split_at(index);
        self.spans.splice(at..at, spans);
        self.normalize();
    }

    /// Removes the rows in `range` (clamped to the list) and returns the spans they were.
    pub fn remove(&mut self, range: Range<u64>) -> Vec<Span> {
        let end = range.end.min(self.len());
        if range.start >= end {
            return Vec::new();
        }
        let first = self.split_at(range.start);
        let last = self.split_at(end);
        let removed = self.spans.drain(first..last).collect();
        self.normalize();
        removed
    }

    /// Removes the rows at `rows` (ascending and distinct; any past the end are ignored) in one
    /// pass, and returns them in order, one span per row. O(s + k) for k rows.
    pub fn remove_rows(&mut self, rows: &[u64]) -> Vec<Span> {
        let mut removed = Vec::with_capacity(rows.len());
        let mut kept = Vec::with_capacity(self.spans.len() + rows.len());
        let mut rows = rows.iter().copied().peekable();
        let mut first = 0;
        for span in self.spans.drain(..) {
            let end = first + span.len;
            // The first row of the span not yet kept or removed.
            let mut from = first;
            while let Some(row) = rows.next_if(|&row| row < end) {
                kept.push(Span::new(span.view.clone(), span.start + from - first, row - from));
                removed.push(Span::new(span.view.clone(), span.start + row - first, 1));
                from = row + 1;
            }
            kept.push(Span { start: span.start + from - first, len: end - from, ..span });
            first = end;
        }
        self.spans = kept;
        self.normalize();
        removed
    }

    /// The row index where span `i` starts.
    fn start_of(&self, i: usize) -> u64 {
        if i == 0 { 0 } else { self.ends[i - 1] }
    }

    /// Drops empty spans, merges adjacent ones and recomputes the running lengths.
    fn normalize(&mut self) {
        let mut merged: Vec<Span> = Vec::with_capacity(self.spans.len());
        for span in self.spans.drain(..).filter(|s| s.len > 0) {
            match merged.last_mut() {
                Some(prev) if prev.view == span.view && prev.start + prev.len == span.start => {
                    prev.len += span.len;
                }
                _ => merged.push(span),
            }
        }
        let mut total = 0;
        self.ends = merged
            .iter()
            .map(|s| {
                total += s.len;
                total
            })
            .collect();
        self.spans = merged;
    }
}

/// Serializes as the spans alone.
impl Serialize for SpanList {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.spans.serialize(serializer)
    }
}

/// Reads the spans and rebuilds the running lengths, dropping empty spans and merging neighbours.
impl<'de> Deserialize<'de> for SpanList {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_spans(Vec::<Span>::deserialize(deserializer)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A [`View::Key`] for `k`.
    fn key(k: &str) -> View {
        View::Key(k.to_string())
    }

    /// A span of `len` positions of the view keyed `k`, from `start`.
    fn span(k: &str, start: u64, len: u64) -> Span {
        Span::new(key(k), start, len)
    }

    /// The rows a list reads, one `(view, position)` per row.
    fn rows(list: &SpanList) -> Vec<(View, u64)> {
        list.spans()
            .iter()
            .flat_map(|s| (s.start..s.start + s.len).map(|p| (s.view.clone(), p)))
            .collect()
    }

    /// Checks the invariants: running lengths, no empty span, no mergeable neighbours.
    fn assert_invariants(list: &SpanList) {
        let mut total = 0;
        for (i, s) in list.spans().iter().enumerate() {
            assert!(s.len > 0, "empty span at {i}");
            total += s.len;
            assert_eq!(list.ends[i], total, "running length at {i}");
            if i > 0 {
                let p = &list.spans()[i - 1];
                assert!(
                    !(p.view == s.view && p.start + p.len == s.start),
                    "mergeable neighbours at {i}"
                );
            }
        }
        assert_eq!(list.ends.len(), list.spans().len());
        assert_eq!(list.len(), total);
    }

    #[test]
    fn span_list_lookup_at_boundaries() {
        let list = SpanList::from_spans([span("a", 10, 3), span("b", 0, 1), span("a", 0, 2)]);
        assert_eq!(list.len(), 6);
        assert_eq!(list.locate(0), Some((0, 0)));
        assert_eq!(list.locate(2), Some((0, 2)));
        assert_eq!(list.locate(3), Some((1, 0)));
        assert_eq!(list.locate(4), Some((2, 0)));
        assert_eq!(list.locate(5), Some((2, 1)));
        assert_eq!(list.locate(6), None);
        assert_eq!(list.get(2), Some((&key("a"), 12)));
        assert_eq!(list.get(5), Some((&key("a"), 1)));
        assert_eq!(SpanList::new().locate(0), None);
    }

    #[test]
    fn span_list_edits_keep_prefix_sums() {
        let mut list = SpanList::from_spans([span("a", 0, 10), span("b", 0, 5)]);
        let before = rows(&list);

        // A split changes no row.
        assert_eq!(list.split_at(4), 1);
        assert_eq!(list.split_at(4), 1);
        assert_eq!(list.split_at(0), 0);
        assert_eq!(list.split_at(15), 3);
        assert_eq!(list.split_at(99), 3);
        assert_eq!(rows(&list), before);
        assert_eq!(list.spans().len(), 3);
        assert_eq!(list.ends, [4, 10, 15]);

        // A splice puts rows where asked, and an empty span is dropped.
        list.splice(10, [span("c", 7, 2), span("x", 0, 0), span("d", 0, 1)]);
        assert_invariants(&list);
        assert_eq!(list.len(), 18);
        assert_eq!(list.get(10), Some((&key("c"), 7)));
        assert_eq!(list.get(12), Some((&key("d"), 0)));
        assert_eq!(list.get(13), Some((&key("b"), 0)));

        // A removal returns what left, and the rest closes up.
        let removed = list.remove(2..12);
        assert_eq!(removed, [span("a", 2, 8), span("c", 7, 2)]);
        assert_invariants(&list);
        assert_eq!(list.len(), 8);
        assert_eq!(list.get(1), Some((&key("a"), 1)));
        assert_eq!(list.get(2), Some((&key("d"), 0)));

        // Out-of-range removals clamp, and an empty range does nothing.
        assert!(list.remove(5..5).is_empty());
        assert!(list.remove(50..60).is_empty());
        list.remove(3..100);
        assert_invariants(&list);
        assert_eq!(list.len(), 3);
        list.remove(0..3);
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn remove_rows_matches_removing_one_row_at_a_time() {
        let list = SpanList::from_spans([span("a", 4, 3), span("b", 0, 1), span("a", 0, 3)]);
        // Every set of rows, plus one past the end.
        for mask in 0u32..1 << 8 {
            let rows: Vec<u64> = (0..8).filter(|row| mask & 1 << row != 0).collect();
            let mut one_pass = list.clone();
            let mut one_by_one = list.clone();
            let mut expected: Vec<Span> =
                rows.iter().rev().flat_map(|&row| one_by_one.remove(row..row + 1)).collect();
            expected.reverse();
            assert_eq!(one_pass.remove_rows(&rows), expected, "rows {rows:?}");
            assert_eq!(one_pass, one_by_one, "rows {rows:?}");
            assert_invariants(&one_pass);
        }
    }

    #[test]
    fn find_sorted_matches_find() {
        let list = SpanList::from_spans([
            span("a", 4, 3),
            span("b", 0, 1),
            Span::new(View::Shuffle, 0, 2),
            span("a", 0, 3),
            span("a", 5, 1),
        ]);
        let mut items: Vec<(&str, u64)> =
            ["a", "b", "c"].iter().flat_map(|&k| (0..9).map(move |pos| (k, pos))).collect();
        items.sort_unstable();
        let expected: Vec<Option<u64>> =
            items.iter().map(|&(k, pos)| list.find(&key(k), pos)).collect();
        assert_eq!(list.find_sorted(&items), expected);
        // Position 5 of `a` is read twice: the first row counts.
        assert_eq!(list.find(&key("a"), 5), Some(1));
        assert!(SpanList::new().find_sorted(&items).iter().all(Option::is_none));
    }

    #[test]
    fn span_list_rows_past_u32() {
        let big = 5_000_000_000;
        let mut list = SpanList::from_spans([span("a", 0, big), span("b", 0, big)]);
        assert_eq!(list.len(), 2 * big);
        assert_eq!(list.locate(big - 1), Some((0, big - 1)));
        assert_eq!(list.locate(big), Some((1, 0)));
        assert_eq!(list.get(big + 4_294_967_296), Some((&key("b"), 4_294_967_296)));
        assert_eq!(list.locate(2 * big), None);

        let removed = list.remove(big - 1..big + 1);
        assert_eq!(removed, [span("a", big - 1, 1), span("b", 0, 1)]);
        assert_eq!(list.len(), 2 * big - 2);
        assert_eq!(list.locate(big - 1), Some((1, 0)));
        assert_invariants(&list);
    }

    #[test]
    fn adjacent_spans_merge() {
        // Contiguous positions of one view merge; a gap, or a different view (a key versus the shuffle) doesn't.
        let list = SpanList::from_spans([
            span("a", 0, 2),
            span("a", 2, 3),
            span("a", 6, 1),
            span("b", 7, 1),
            Span::new(View::Shuffle, 0, 1),
            Span::new(View::Shuffle, 1, 1),
        ]);
        assert_eq!(
            list.spans(),
            [span("a", 0, 5), span("a", 6, 1), span("b", 7, 1), Span::new(View::Shuffle, 0, 2),]
        );
        assert_invariants(&list);

        // Removing the middle of a span and putting it back leaves one span again.
        let mut list = SpanList::from_spans([span("a", 0, 10)]);
        let removed = list.remove(3..6);
        assert_eq!(list.spans().len(), 2);
        list.splice(3, removed);
        assert_eq!(list.spans(), [span("a", 0, 10)]);
        assert_invariants(&list);

        // Removing what separates two spans of one view joins them.
        let mut list = SpanList::from_spans([span("a", 0, 2), span("b", 0, 1), span("a", 2, 2)]);
        list.remove(2..3);
        assert_eq!(list.spans(), [span("a", 0, 4)]);
        assert_invariants(&list);
    }
}
