//! History: plays recorded as runs, read newest first, removed and re-queued
//! (`design/queue.md` §5, §6).

use super::edit::PlayRef;
use super::read::Row;
use super::spans::{Span, SpanList, View};
use super::state::{ItemId, QueueCopy, QueueError, QueueState, Run};

/// How many runs History keeps. A core constant so every device trims as the server does.
pub const HISTORY_RUNS: usize = 1_000;

/// One play of History: its number and the row it played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayRow<'a> {
    /// The play's number.
    pub play: u64,
    /// The row played.
    pub row: Row<'a>,
}

/// The span of one position that `item` names.
pub(super) fn play_span(item: &ItemId) -> Span {
    Span::new(View::Key(item.key.clone()), item.pos, 1)
}

impl QueueState {
    /// The spans naming the source position `item` reads, for a flattened copy of it.
    pub(super) fn source_spans(&self, item: &ItemId) -> Vec<Span> {
        match self.copies.iter().find(|c| c.key == item.key) {
            Some(copy) => copy.of.slice(item.pos..item.pos + 1),
            None => vec![play_span(item)],
        }
    }

    /// Records `spans`, in order, as consecutive plays: each extends the last run when it
    /// continues it and starts a run otherwise. Then trims History. Returns whether the trim
    /// dropped runs, the only way recording lets go of a view: the caller then cleans up.
    pub(super) fn record_plays(&mut self, spans: impl IntoIterator<Item = Span>) -> bool {
        for span in spans {
            let first = self.next_play;
            self.next_play += span.len;
            match self.history.last_mut() {
                Some(run)
                    if run.first + run.span.len == first
                        && run.span.view == span.view
                        && run.span.start + run.span.len == span.start =>
                {
                    run.span.len += span.len;
                }
                _ => self.history.push(Run { first, span }),
            }
        }
        self.trim_history()
    }

    /// Drops the oldest runs past [`HISTORY_RUNS`], and says whether any went. Call it after
    /// anything that can add runs.
    pub(super) fn trim_history(&mut self) -> bool {
        let excess = self.history.len().saturating_sub(HISTORY_RUNS);
        self.history.drain(..excess);
        excess > 0
    }

    /// The number of plays in History.
    pub fn history_len(&self) -> u64 {
        self.history.iter().map(|run| run.span.len).sum()
    }

    /// History row `index`, newest first, or `None` past the end or where the row can't be
    /// read. O(runs).
    pub fn history_row(&self, index: u64) -> Option<PlayRow<'_>> {
        let mut skip = index;
        for run in self.history.iter().rev() {
            if skip < run.span.len {
                let offset = run.span.len - 1 - skip;
                let View::Key(key) = &run.span.view else { return None };
                let row = self.row_of(key, run.span.start + offset)?;
                return Some(PlayRow { play: run.first + offset, row });
            }
            skip -= run.span.len;
        }
        None
    }

    /// The run holding play `play` and the play's offset in it.
    fn find_play(&self, play: u64) -> Option<(usize, u64)> {
        let i = self.history.partition_point(|run| run.first + run.span.len <= play);
        let run = self.history.get(i)?;
        (run.first <= play).then(|| (i, play - run.first))
    }

    /// The run and offset of `play` if it is in History and played the item it names.
    fn match_play(&self, play: &PlayRef) -> Option<(usize, u64)> {
        let (i, offset) = self.find_play(play.play)?;
        let span = &self.history[i].span;
        let View::Key(key) = &span.view else { return None };
        (play.item.key == *key && play.item.pos == span.start + offset).then_some((i, offset))
    }

    /// Takes play `offset` of run `i` out of History, splitting the run, then trims since a
    /// split can add a run. Play numbers stay as they were, so the gap is never reused.
    fn take_play(&mut self, i: usize, offset: u64) {
        let run = self.history.remove(i);
        let after = offset + 1;
        let tail = Run {
            first: run.first + after,
            span: Span::new(run.span.view.clone(), run.span.start + after, run.span.len - after),
        };
        let head = Run { first: run.first, span: Span { len: offset, ..run.span } };
        let kept = [head, tail].into_iter().filter(|r| r.span.len > 0);
        self.history.splice(i..i, kept);
        self.trim_history();
    }

    /// Removes the plays that are in History and played the item named, ignoring the rest.
    /// Sets `adjusted` if any went.
    pub(super) fn remove_plays(&mut self, plays: &[PlayRef]) {
        let mut any = false;
        for play in plays {
            if let Some((i, offset)) = self.match_play(play) {
                self.take_play(i, offset);
                any = true;
            }
        }
        if any {
            self.adjusted = true;
            self.clean_up();
        }
    }

    /// Puts a copy of the played row, under `key`, at the front of the Manual Queue and takes
    /// the play out of History. A play that isn't in History or played another item is
    /// ignored; a `key` already in use fails the edit.
    pub(super) fn requeue(&mut self, play: &PlayRef, key: &str) -> Result<(), QueueError> {
        let Some((i, offset)) = self.match_play(play) else { return Ok(()) };
        if self.has_key(key) {
            return Err(QueueError::DuplicateKey);
        }
        let of = self.source_spans(&play.item);
        self.take_play(i, offset);
        self.copies.push(QueueCopy { key: key.to_string(), of: SpanList::from_spans(of) });
        self.manual.splice(0, [Span::new(View::Key(key.to_string()), 0, 1)]);
        self.adjusted = true;
        self.clean_up();
        Ok(())
    }
}
