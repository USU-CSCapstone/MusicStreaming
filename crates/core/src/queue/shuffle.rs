//! Shuffle and unshuffle (`design/queue.md` §4, §6).
//!
//! While shuffled, `order` is the rows passed before the shuffle, then `Shuffle[0..m)` over the
//! frozen base, and edits split and move those spans like any others. Everything that is not
//! the shuffled context records list positions, so a shuffle row never reaches the Manual
//! Queue, History or a copy.

use super::history::{HISTORY_RUNS, play_span};
use super::permutation::{permute, unpermute};
use super::spans::{Span, SpanList, View};
use super::state::{QueueState, Shuffle};

/// Where a `Shuffle` span of `order` sits: its first shuffle position, its length and its row
/// in `order`. Sorted by first shuffle position, so a lookup is a binary search.
type ShuffleRows = Vec<(u64, u64, u64)>;

/// The `order` row of shuffle position `p`, or `None` if no row reads it (it was removed or
/// moved out).
fn row_of_position(rows: &ShuffleRows, p: u64) -> Option<u64> {
    let i = rows.partition_point(|&(start, _, _)| start <= p).checked_sub(1)?;
    let (start, len, row) = rows[i];
    (p < start + len).then(|| row + (p - start))
}

/// The parts of positions `start..end` of view `key` that `base` holds, as (first position,
/// count, base row of the first).
fn base_overlaps(base: &SpanList, key: &str, start: u64, end: u64) -> Vec<(u64, u64, u64)> {
    let mut out = Vec::new();
    let mut row = 0;
    for span in base.spans() {
        if matches!(&span.view, View::Key(k) if k == key) {
            let (from, to) = (span.start.max(start), (span.start + span.len).min(end));
            if from < to {
                out.push((from, to - from, row + from - span.start));
            }
        }
        row += span.len;
    }
    out
}

/// The rows `from..to` of `base` except those in `removed` (ascending), as spans.
fn base_without(base: &SpanList, removed: &[u64], from: u64, to: u64) -> Vec<Span> {
    let mut out = Vec::new();
    let mut at = from;
    for &row in &removed[removed.partition_point(|&r| r < from)..] {
        if row >= to {
            break;
        }
        out.extend(base.slice(at..row));
        at = row + 1;
    }
    out.extend(base.slice(at..to));
    out
}

impl QueueState {
    /// Turns shuffle on or off. Turning it on freezes the unplayed context as the base, and
    /// Up Next's context part becomes `shuffle[0..m)`. Turning it on while shuffled, or off
    /// while not, does nothing: a shuffled order is fixed once generated.
    pub(super) fn set_shuffle(&mut self, on: bool, seed: u32) {
        match (on, self.shuffle.is_some()) {
            (true, false) => {
                let base = SpanList::from_spans(self.order.remove(self.cursor..self.order.len()));
                self.order.splice(self.order.len(), [Span::new(View::Shuffle, 0, base.len())]);
                self.shuffle = Some(Shuffle { seed, base });
            }
            (false, true) => self.unshuffle(),
            _ => {}
        }
    }

    /// At a Repeat All restart: a shuffle with an empty base was turned on with nothing left to
    /// play, so the lap about to start is shuffled. The whole context becomes the base and
    /// `order` reads it through `Shuffle[0..m)`, with the cursor still at the restart. Until
    /// then the context is as it was, so turning shuffle off in the meantime changes nothing,
    /// and rows added meanwhile are part of the lap.
    pub(super) fn shuffle_next_lap(&mut self) {
        let Some(shuffle) = self.shuffle.as_mut().filter(|s| s.base.is_empty()) else { return };
        shuffle.base = std::mem::take(&mut self.order);
        self.order = SpanList::from_spans([Span::new(View::Shuffle, 0, shuffle.base.len())]);
    }

    /// `spans` with every shuffle row replaced by the list position it plays, so what is
    /// recorded outside the shuffled context never reads the shuffle. One span per row, merged
    /// with a neighbour it continues. O(spans + shuffle rows·π).
    pub(super) fn list_spans(&self, spans: Vec<Span>) -> Vec<Span> {
        let mut out = Vec::with_capacity(spans.len());
        for span in spans {
            match span.view {
                View::Key(_) => out.push(span),
                View::Shuffle => out.extend(
                    (span.start..span.start + span.len)
                        .filter_map(|p| self.item_of(&span.view, p).as_ref().map(play_span)),
                ),
            }
        }
        SpanList::from_spans(out).spans().to_vec()
    }

    /// `spans` as the plays they make, in order, with each shuffle row as the list position it
    /// plays. Only the newest plays History can keep are computed: also returns how many plays
    /// come before them, which only advance `nextPlay`. O(spans + min(rows, `HISTORY_RUNS`)·π).
    pub(super) fn played(&self, spans: Vec<Span>) -> (u64, Vec<Span>) {
        if spans.iter().all(|s| s.view != View::Shuffle) {
            return (0, spans);
        }
        let total: u64 = spans.iter().map(|s| s.len).sum();
        // Newest first: an older play merges into the newer run it continues.
        let mut newest: Vec<Span> = Vec::new();
        let mut push = |piece: Span| {
            match newest.last_mut() {
                Some(next) if next.view == piece.view && piece.start + piece.len == next.start => {
                    next.start = piece.start;
                    next.len += piece.len;
                }
                _ => newest.push(piece),
            }
            newest.len() > HISTORY_RUNS
        };
        for span in spans.iter().rev() {
            let full = match span.view {
                View::Key(_) => push(span.clone()),
                View::Shuffle => (span.start..span.start + span.len).rev().any(|p| {
                    let played = self.item_of(&span.view, p);
                    played.is_some_and(|item| push(play_span(&item)))
                }),
            };
            if full {
                break;
            }
        }
        let kept: u64 = newest.iter().map(|s| s.len).sum();
        newest.reverse();
        (total - kept, newest)
    }

    /// [`QueueState::order_row_of`] for each of `items` (sorted by key, then position), in one
    /// sweep over `order` and, while shuffled, one over the base: O((s + b) log k + k·π).
    pub(super) fn order_rows_of(&self, items: &[(&str, u64)]) -> Vec<Option<u64>> {
        let mut rows = self.order.find_sorted(items);
        let Some(shuffle) = &self.shuffle else { return rows };
        let (shuffled, m) = (self.shuffle_rows(), shuffle.base.len());
        for (row, base) in rows.iter_mut().zip(shuffle.base.find_sorted(items)) {
            if row.is_none() {
                *row = base.and_then(|b| row_of_position(&shuffled, unpermute(shuffle.seed, m, b)));
            }
        }
        rows
    }

    /// The `Shuffle` spans of `order`, by shuffle position.
    fn shuffle_rows(&self) -> ShuffleRows {
        let mut rows = ShuffleRows::new();
        let mut row = 0;
        for span in self.order.spans() {
            if span.view == View::Shuffle {
                rows.push((span.start, span.len, row));
            }
            row += span.len;
        }
        rows.sort_unstable();
        rows
    }

    /// The base rows whose shuffle position no `Shuffle` span of `order` reads, ascending: rows
    /// removed or moved out while shuffled.
    fn gone_rows(rows: &ShuffleRows, seed: u32, m: u64) -> Vec<u64> {
        let mut gone = Vec::new();
        let mut at = 0;
        for &(start, len, _) in rows {
            gone.extend((at..start).map(|p| permute(seed, m, p)));
            at = start + len;
        }
        gone.extend((at..m).map(|p| permute(seed, m, p)));
        gone.sort_unstable();
        gone
    }

    /// Which of the positions `ranges` (view key, first, end; disjoint) are in Up Next through
    /// the shuffle once the cursor is at row `from`: those the base holds whose shuffle row is
    /// at `from` or after. Returned as ranges. Costs min(positions in the base, rows that
    /// aren't held) times π, so neither a long run nor a long jump makes a rewind slow alone.
    pub(super) fn shuffle_held(
        &self,
        from: u64,
        ranges: &[(String, u64, u64)],
    ) -> Vec<(String, u64, u64)> {
        self.shuffle_held_by(from, ranges, None)
    }

    /// [`Self::shuffle_held`] with the strategy named: `Some(true)` looks up each position of
    /// the base the ranges cover, `Some(false)` finds the rows that aren't held and subtracts
    /// them. Both give the same ranges.
    pub(super) fn shuffle_held_by(
        &self,
        from: u64,
        ranges: &[(String, u64, u64)],
        lookup: Option<bool>,
    ) -> Vec<(String, u64, u64)> {
        let Some(Shuffle { seed, base }) = &self.shuffle else { return Vec::new() };
        let (seed, m) = (*seed, base.len());
        let overlaps: Vec<(&str, u64, u64, u64)> = ranges
            .iter()
            .flat_map(|(key, start, end)| {
                let found = base_overlaps(base, key, *start, *end);
                found.into_iter().map(move |(q, n, b)| (key.as_str(), q, n, b))
            })
            .collect();
        let rows = self.shuffle_rows();
        let looked_up: u64 = overlaps.iter().map(|o| o.2).sum();
        let covered: u64 = rows.iter().map(|r| r.1).sum();
        let passed: u64 = rows.iter().map(|&(_, len, row)| from.saturating_sub(row).min(len)).sum();

        let mut held = Vec::new();
        if lookup.unwrap_or(looked_up <= m - covered + passed) {
            for (key, q, n, b) in overlaps {
                let mut open: Option<u64> = None;
                for i in 0..=n {
                    let is_held = i < n
                        && row_of_position(&rows, unpermute(seed, m, b + i))
                            .is_some_and(|row| row >= from);
                    match (is_held, open) {
                        (true, None) => open = Some(i),
                        (false, Some(first)) => {
                            held.push((key.to_string(), q + first, q + i));
                            open = None;
                        }
                        _ => {}
                    }
                }
            }
        } else {
            let mut unheld = Self::gone_rows(&rows, seed, m);
            for &(start, len, row) in &rows {
                let passed = start..start + from.saturating_sub(row).min(len);
                unheld.extend(passed.map(|p| permute(seed, m, p)));
            }
            unheld.sort_unstable();
            for (key, q, n, b) in overlaps {
                let mut at = b;
                for &gap in &unheld[unheld.partition_point(|&u| u < b)..] {
                    if gap >= b + n {
                        break;
                    }
                    if gap > at {
                        held.push((key.to_string(), q + at - b, q + gap - b));
                    }
                    at = gap + 1;
                }
                if at < b + n {
                    held.push((key.to_string(), q + at - b, q + n));
                }
            }
        }
        held
    }

    /// Puts the context back in its natural order and drops the shuffle (`design/queue.md` §6).
    ///
    /// `order` becomes the rows before the first shuffle row, then the base in natural order
    /// less the rows removed or moved out while shuffled, with the cursor just after the pivot:
    /// the playing item if the base holds it, otherwise the latest passed shuffle row. Rows
    /// the shuffle never read are additions: one in Up Next goes just before the natural place
    /// of the base row that followed it (the front of Up Next if that place is passed, the end
    /// if no base row followed), and one already passed stays passed, just before the cursor,
    /// in the order passed. A Manual Queue track that also holds a passed shuffle row, and
    /// whose natural place is after the pivot, is left out of Up Next and keeps a passed place
    /// there too. Costs O(spans + base spans + edits made while shuffled) and π for each of
    /// those edits.
    fn unshuffle(&mut self) {
        let Some(Shuffle { seed, base }) = &self.shuffle else { return };
        let (seed, m, cursor) = (*seed, base.len(), self.cursor);
        let rows = self.shuffle_rows();

        // Sort the context's key rows: the head before the first shuffle row, passed additions
        // after it, and the rows still ahead, in order, with the first base row of each shuffle
        // span ahead (what the additions are put before).
        enum Ahead {
            Row { span: Span, head: bool },
            Base(u64),
        }
        let mut head = Vec::new();
        let mut passed = Vec::new();
        let mut ahead = Vec::new();
        let mut at = 0;
        let mut seen_shuffle = false;
        for span in self.order.spans() {
            let end = at + span.len;
            if span.view == View::Shuffle {
                seen_shuffle = true;
                if end > cursor {
                    ahead.push(Ahead::Base(permute(seed, m, span.start + cursor.max(at) - at)));
                }
            } else {
                let cut = cursor.clamp(at, end);
                let part = |from: u64, to: u64| {
                    Span::new(span.view.clone(), span.start + from - at, to - from)
                };
                if at < cut {
                    let behind = if seen_shuffle { &mut passed } else { &mut head };
                    behind.push((at, part(at, cut)));
                }
                if cut < end {
                    ahead.push(Ahead::Row { span: part(cut, end), head: !seen_shuffle });
                }
            }
            at = end;
        }

        // The pivot, as a base row.
        let in_base =
            self.current.as_ref().and_then(|c| base.find(&View::Key(c.key.clone()), c.pos));
        let latest_passed = rows
            .iter()
            .filter(|&&(_, _, row)| row < cursor)
            .max_by_key(|&&(_, _, row)| row)
            .map(|&(start, len, row)| permute(seed, m, start + (cursor - row).min(len) - 1));
        let pivot = in_base.or(latest_passed);

        // Base rows left out: removed or moved out, and Manual Queue tracks whose passed place
        // is a shuffle row that natural order would put after the pivot.
        let mut left_out = Self::gone_rows(&rows, seed, m);
        for span in self.manual.spans() {
            let View::Key(key) = &span.view else { continue };
            for (q, n, b) in base_overlaps(base, key, span.start, span.start + span.len) {
                for i in 0..n {
                    let p = unpermute(seed, m, b + i);
                    let after = pivot.is_none_or(|j| b + i > j);
                    if let (Some(row), true) = (row_of_position(&rows, p), after) {
                        passed.push((row, Span::new(span.view.clone(), q + i, 1)));
                        left_out.push(b + i);
                    }
                }
            }
        }
        left_out.sort_unstable();
        passed.sort_by_key(|&(row, _)| row);

        // Rebuild.
        let split = pivot.map_or(0, |j| j + 1);
        let natural = base_without(base, &left_out, 0, split);
        let mut order: Vec<Span> = head.into_iter().map(|(_, span)| span).collect();
        order.extend(natural);
        order.extend(passed.into_iter().map(|(_, span)| span));
        let new_cursor = order.iter().map(|s| s.len).sum();
        // An addition still ahead goes just before the natural place of the base row that
        // followed it, but not into the passed part; one with none following goes last. A
        // head row still ahead stays at the front.
        let mut anchored: Vec<(u64, Span)> = Vec::new();
        let mut last = Vec::new();
        let mut following = None;
        for entry in ahead.into_iter().rev() {
            match entry {
                Ahead::Base(b) => following = Some(b),
                Ahead::Row { span, head: true } => anchored.push((split, span)),
                Ahead::Row { span, .. } => match following {
                    Some(b) => anchored.push((b.max(split), span)),
                    None => last.push(span),
                },
            }
        }
        anchored.reverse();
        last.reverse();
        anchored.sort_by_key(|&(anchor, _)| anchor);
        let mut up_next = Vec::new();
        let mut from = split;
        for (anchor, span) in anchored {
            up_next.extend(base_without(base, &left_out, from, anchor));
            up_next.push(span);
            from = anchor;
        }
        up_next.extend(base_without(base, &left_out, from, m));
        up_next.extend(last);
        let up_next = SpanList::from_spans(up_next);
        order.extend(up_next.spans().iter().cloned());
        self.order = SpanList::from_spans(order);
        self.cursor = new_cursor;
        self.shuffle = None;
        self.clean_up();
    }
}
