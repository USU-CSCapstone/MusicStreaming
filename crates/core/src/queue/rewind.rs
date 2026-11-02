//! Rewind: Previous repeated until a chosen play is current (`design/queue.md` §6).

use std::collections::BTreeMap;

use super::edit::PlayRef;
use super::history::play_span;
use super::spans::{Span, SpanList, View};
use super::state::{ItemId, QueueCopy, QueueError, QueueState};

/// Consecutive positions of one view, and whether they need the copy.
struct Piece {
    key: String,
    start: u64,
    len: u64,
    copy: bool,
}

impl Piece {
    /// A stretch of original positions.
    fn new(key: &str, start: u64, len: u64) -> Self {
        Self { key: key.to_string(), start, len, copy: false }
    }
}

/// The positions that Up Next holds, per view: disjoint ranges keyed by (view key, start), each
/// valued by its end.
type Held = BTreeMap<(String, u64), u64>;

/// Splits `piece` into the parts Up Next already holds, marked as copies, and the rest, in
/// position order, and then counts the rest as held.
fn split_held(held: &mut Held, piece: &Piece) -> Vec<Piece> {
    let end = piece.start + piece.len;
    let key = &piece.key;
    let before = held.range((key.clone(), 0)..(key.clone(), piece.start)).next_back();
    let overlaps: Vec<(u64, u64)> = before
        .filter(|(_, e)| **e > piece.start)
        .into_iter()
        .chain(held.range((key.clone(), piece.start)..(key.clone(), end)))
        .map(|((_, s), e)| (*s, *e))
        .collect();

    let mut parts = Vec::new();
    let mut at = piece.start;
    for (s, e) in overlaps {
        let (from, to) = (s.max(at), e.min(end));
        if from > at {
            parts.push(Piece::new(key, at, from - at));
        }
        parts.push(Piece { copy: true, ..Piece::new(key, from, to - from) });
        at = to;
    }
    if at < end {
        parts.push(Piece::new(key, at, end - at));
    }
    for part in parts.iter().filter(|p| !p.copy) {
        held.insert((key.clone(), part.start), part.start + part.len);
    }
    parts
}

impl QueueState {
    /// Goes back to play `play`, with the result of Previous repeated until it is current: it
    /// plays, then everything heard after it in the order heard, then the item that was playing,
    /// then Up Next as it was. The plays leave History, and `nextPlay` is left alone.
    ///
    /// Phase 1, only if the Manual Queue is empty: the cursor moves back over the longest
    /// stretch, the item playing and then the newest plays, that `order` read backwards from
    /// the cursor matches. Phase 2: the rest go to the front of the Manual Queue in the order
    /// heard, each as a span of its own view, keeping any passed place in `order`, except the
    /// plays whose items are in Up Next by then. Those are flattened into one copy under `key`,
    /// which also plays if the target itself is one. Under a shuffle, a play is a copy while its
    /// item still has a shuffle row ahead of the cursor, found by π⁻¹ for each play of a run that
    /// the base holds, or by π of each row not ahead if that is fewer. Costs O((r + s) log s)
    /// for r runs and s spans, plus π for the rows just named. Fails with
    /// [`QueueError::PlayGone`] if `play` isn't in History or played another item, and with
    /// [`QueueError::DuplicateKey`] if a copy is needed and `key` is in use, changing nothing
    /// either way. Never sets `adjusted`, and needs no clean-up: every view a removed play
    /// named is still named by Up Next or the playing item.
    pub(super) fn rewind(&mut self, play: &PlayRef, key: &str) -> Result<(), QueueError> {
        let (first_run, offset) = self.match_play(play).ok_or(QueueError::PlayGone)?;

        // The target, then the plays after it as stretches in the order heard.
        let mut heard = Vec::new();
        for (n, run) in self.history[first_run..].iter().enumerate() {
            let View::Key(run_key) = &run.span.view else { return Ok(()) };
            let skip = if n == 0 { offset } else { 0 };
            heard.push(Piece::new(run_key, run.span.start + skip, run.span.len - skip));
        }
        let target = Piece::new(&heard[0].key, heard[0].start, 1);
        heard[0].start += 1;
        heard[0].len -= 1;

        // Phase 1.
        let old = self.current.clone();
        let mut row = self.cursor;
        let mut old_left = old.clone();
        let mut walking = self.manual.is_empty();
        if let (true, Some(old)) = (walking, &old) {
            let mut left = 1;
            self.match_back(&old.key, old.pos, &mut left, &mut row);
            if left == 0 {
                old_left = None;
            } else {
                walking = false;
            }
        }
        if walking {
            for piece in heard.iter_mut().rev() {
                let mut left = piece.len;
                self.match_back(&piece.key, piece.start, &mut left, &mut row);
                piece.len = left;
                if left > 0 {
                    break;
                }
            }
        }
        heard.retain(|piece| piece.len > 0);

        // What Up Next holds once phase 1 is done, and the plays that must be copies.
        let mut held = Held::new();
        let mut hold = |view: &View, start: u64, len: u64| {
            if let View::Key(key) = view {
                held.insert((key.clone(), start), start + len);
            }
        };
        for span in self.manual.spans() {
            hold(&span.view, span.start, span.len);
        }
        for span in self.order.slice(row..self.order.len()) {
            hold(&span.view, span.start, span.len);
        }
        // Context rows read through a shuffle are held where their shuffle row is still ahead.
        for (key, start, end) in self.shuffle_held(row, &Self::positions_of(&target, &heard)) {
            hold(&View::Key(key), start, end - start);
        }
        if let Some(old) = &old_left {
            hold(&View::Key(old.key.clone()), old.pos, 1);
        }
        let mut parts: Vec<Vec<Piece>> = std::iter::once(&target)
            .chain(&heard)
            .rev()
            .map(|piece| split_held(&mut held, piece))
            .collect();
        parts.reverse();
        let copies = parts.iter().flatten().any(|part| part.copy);
        if copies && self.has_key(key) {
            return Err(QueueError::DuplicateKey);
        }

        // Build the copy and the spans for the Manual Queue, in the order heard.
        let mut of = Vec::new();
        let mut copy_len = 0;
        let mut queued = Vec::new();
        let mut playing = None;
        for (n, part) in parts.iter().flatten().enumerate() {
            let (view_key, start) = if part.copy {
                of.extend(self.source_range(&part.key, part.start, part.len));
                copy_len += part.len;
                (key, copy_len - part.len)
            } else {
                (part.key.as_str(), part.start)
            };
            // The target is the first row heard: it plays, and the rest of its part is queued.
            let skip = u64::from(n == 0);
            if n == 0 {
                playing = Some(ItemId { key: view_key.to_string(), pos: start });
            }
            if part.len > skip {
                queued.push(Span::new(
                    View::Key(view_key.to_string()),
                    start + skip,
                    part.len - skip,
                ));
            }
        }
        queued.extend(old_left.as_ref().map(play_span));

        // Apply.
        let run = &mut self.history[first_run];
        run.span.len = offset;
        let keep = first_run + usize::from(offset > 0);
        self.history.truncate(keep);
        self.cursor = row;
        self.manual.splice(0, queued);
        self.current = playing;
        if copies {
            self.copies.push(QueueCopy { key: key.to_string(), of: SpanList::from_spans(of) });
        }
        Ok(())
    }

    /// The positions the target and the plays heard after it cover, per view, as disjoint
    /// ranges (view key, first, end).
    fn positions_of(target: &Piece, heard: &[Piece]) -> Vec<(String, u64, u64)> {
        let mut ranges: Vec<(String, u64, u64)> = std::iter::once(target)
            .chain(heard)
            .map(|p| (p.key.clone(), p.start, p.start + p.len))
            .collect();
        ranges.sort_unstable();
        let mut merged: Vec<(String, u64, u64)> = Vec::new();
        for range in ranges {
            match merged.last_mut() {
                Some(last) if last.0 == range.0 && range.1 <= last.2 => {
                    last.2 = last.2.max(range.2)
                }
                _ => merged.push(range),
            }
        }
        merged
    }

    /// Matches the last `*left` positions of `key` from `start` against the context rows before
    /// row `*row`, read backwards, taking the rows as far as they match. Lowers `*left` and
    /// `*row` by what matched.
    fn match_back(&self, key: &str, start: u64, left: &mut u64, row: &mut u64) {
        while *left > 0 && *row > 0 {
            let Some((i, offset)) = self.order.locate(*row - 1) else { return };
            let span = &self.order.spans()[i];
            if span.view == View::Shuffle {
                // A shuffle row plays the base row π gives it, so rows match one at a time.
                let played = self.item_at(&self.order, *row - 1);
                if played != Some(ItemId { key: key.to_string(), pos: start + *left - 1 }) {
                    return;
                }
                *left -= 1;
                *row -= 1;
                continue;
            }
            let same = matches!(&span.view, View::Key(k) if k == key);
            if !same || span.start + offset != start + *left - 1 {
                return;
            }
            let taken = (offset + 1).min(*left);
            *left -= taken;
            *row -= taken;
        }
    }
}
