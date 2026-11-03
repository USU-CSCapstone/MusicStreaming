//! The edits that change a queue's Up Next (`design/queue.md` §5, §6).
//!
//! Edits name items, never positions, so they mean the same thing whatever order they arrive
//! in. They serialize as tagged JSON: they are the wire's `queueEdit` payloads.

use serde::{Deserialize, Serialize};

use super::history::play_span;
use super::spans::{Span, View};
use super::state::{ItemId, QueueError, QueueState, Repeat, Source};

/// Where an add or move lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Anchor {
    /// The front of the Manual Queue: Queue Next.
    ManualStart,
    /// The end of the Manual Queue: Queue Last.
    ManualEnd,
    /// The end of the context's Up Next.
    ContextEnd,
    /// Just ahead of an item in Up Next, in the Manual Queue or the context.
    Before {
        /// The item the rows go ahead of.
        item: ItemId,
    },
}

/// A play of History as an edit names it: acted on only if the number and the item both match,
/// since a device's play numbers are guesses until confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayRef {
    /// The play's number.
    pub play: u64,
    /// The item it played.
    pub item: ItemId,
}

/// One change to a queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[cfg_attr(
    target_pointer_width = "64",
    expect(
        clippy::large_enum_variant,
        reason = "an edit is read, applied and dropped, so an Add's source is not worth a box at every construction"
    )
)]
pub enum Edit {
    /// Register a new source and put all its rows at the anchor, in order.
    Add {
        /// The source, whose key must be new.
        source: Source,
        /// Where its rows go.
        before: Anchor,
    },
    /// Drop items from Up Next. Items that are gone, or aren't in Up Next, are ignored.
    Remove {
        /// The items.
        items: Vec<ItemId>,
    },
    /// Move items from Up Next to the anchor, keeping their order in the queue. Items that are
    /// gone, or aren't in Up Next, are ignored.
    Move {
        /// The items.
        items: Vec<ItemId>,
        /// Where they go.
        before: Anchor,
    },
    /// Play an item of Up Next now: everything ahead of it leaves Up Next.
    Jump {
        /// The item.
        item: ItemId,
    },
    /// Play the next item.
    Next,
    /// Take plays out of History. Plays that are gone or played another item are ignored.
    RemovePlays {
        /// The plays.
        plays: Vec<PlayRef>,
    },
    /// Put a copy of a played item at the front of Up Next and take the play out of History.
    /// Ignored if the play is gone or played another item.
    Requeue {
        /// The play.
        play: PlayRef,
        /// The copy's key, minted by the device and new.
        key: String,
    },
    /// Step back: the last play becomes current and the item playing returns to the front of
    /// Up Next. Does nothing with an empty History.
    Previous {
        /// The key of the copy made when the last play's item is still in Up Next; unused
        /// otherwise, and then it may name anything.
        key: String,
    },
    /// Step back until a play is current, as Previous repeated would: it plays, then everything
    /// heard after it in the order heard, then Up Next as it was. Fails with
    /// [`QueueError::PlayGone`] if the play is gone or played another item.
    Rewind {
        /// The play to go back to.
        play: PlayRef,
        /// The key of the one copy made when some of the plays are still in Up Next; unused
        /// otherwise, and then it may name anything.
        key: String,
    },
    /// Shuffle the unplayed context with `seed`, or put it back in its natural order. Turning
    /// shuffle on while shuffled, or off while not, does nothing.
    Shuffle {
        /// Whether to shuffle.
        on: bool,
        /// Fixes the order; chosen by the device so a shuffle works offline.
        seed: u32,
    },
    /// Set the repeat mode.
    Repeat {
        /// The mode.
        mode: Repeat,
    },
}

/// Where rows are put: an index in the Manual Queue, or in the context's `order`.
#[derive(Clone, Copy)]
enum Spot {
    Manual(u64),
    Order(u64),
}

impl QueueState {
    /// Applies one edit. An edit that fails changes nothing.
    ///
    /// Items an edit names that aren't in Up Next are ignored, and an edit that then does
    /// nothing leaves the state as it was, `adjusted` included. An anchor that isn't in Up Next
    /// fails the whole edit with [`QueueError::AnchorGone`], so a device can revert and say so.
    /// `adjusted` is set by an add, remove, move, play removal or re-queue that acts, and by no
    /// other edit (Previous, rewind, shuffle and repeat are not). A context track that Previous
    /// or rewind put in the Manual Queue holds a second, passed place in the context: the same
    /// item ID in `order` before the cursor. A move within the Manual Queue keeps it; a move
    /// into the context, or a removal, drops it.
    pub fn apply(&mut self, edit: &Edit) -> Result<(), QueueError> {
        match edit {
            Edit::Add { source, before } => self.add(source, before),
            Edit::Remove { items } => {
                self.remove(items);
                Ok(())
            }
            Edit::Move { items, before } => self.move_items(items, before),
            Edit::Jump { item } => {
                self.jump(item);
                Ok(())
            }
            Edit::Next => {
                self.next();
                Ok(())
            }
            Edit::RemovePlays { plays } => {
                self.remove_plays(plays);
                Ok(())
            }
            Edit::Requeue { play, key } => self.requeue(play, key),
            Edit::Previous { key } => self.previous(key),
            Edit::Rewind { play, key } => self.rewind(play, key),
            Edit::Shuffle { on, seed } => {
                self.set_shuffle(*on, *seed);
                Ok(())
            }
            Edit::Repeat { mode } => {
                self.repeat = *mode;
                Ok(())
            }
        }
    }

    /// Registers `source` and puts all its rows at the anchor. A source with no rows is not
    /// registered.
    fn add(&mut self, source: &Source, before: &Anchor) -> Result<(), QueueError> {
        let spot = self.spot(before, &[])?;
        if self.has_key(&source.key) {
            return Err(QueueError::DuplicateKey);
        }
        if source.length == 0 {
            return Ok(());
        }
        let span = Span::new(View::Key(source.key.clone()), 0, source.length);
        self.sources.push(source.clone());
        self.put(spot, vec![span]);
        self.adjusted = true;
        Ok(())
    }

    /// Drops the named items of Up Next, then drops what nothing refers to.
    fn remove(&mut self, items: &[ItemId]) {
        let at = self.resolve(items);
        if at.is_empty() {
            return;
        }
        self.take(&at, false);
        self.adjusted = true;
        self.clean_up();
    }

    /// Moves the named items of Up Next to the anchor.
    fn move_items(&mut self, items: &[ItemId], before: &Anchor) -> Result<(), QueueError> {
        let at = self.resolve(items);
        let target = self.spot(before, &at)?;
        if at.is_empty() {
            return Ok(());
        }
        let to_manual = matches!(target, Spot::Manual(_));
        let mut moved = self.take(&at, to_manual);
        if to_manual {
            // The Manual Queue records list positions, never shuffle rows.
            moved = self.list_spans(moved);
        }
        // Taking the rows shifted every position, so the anchor is found again.
        let spot = self.spot(before, &[])?;
        self.put(spot, moved);
        self.adjusted = true;
        Ok(())
    }

    /// Plays `item` now. Everything ahead of it in Up Next leaves it and, like the item that
    /// was playing, becomes plays: one run per span passed. Does nothing if `item` isn't in
    /// Up Next.
    fn jump(&mut self, item: &ItemId) {
        let Some(index) = self.find_item(item) else { return };
        let mut plays: Vec<Span> = self.current.iter().map(play_span).collect();
        match index.checked_sub(self.manual.len()) {
            None => {
                plays.extend(self.manual.remove(0..index));
                self.manual.remove(0..1);
            }
            Some(rest) => {
                plays.extend(self.manual.remove(0..self.manual.len()));
                plays.extend(self.order.slice(self.cursor..self.cursor + rest));
                self.cursor += rest + 1;
            }
        }
        self.current = Some(item.clone());
        if self.record_plays(plays) {
            self.clean_up();
        }
    }

    /// Plays the front of the Manual Queue, or else the next context row, and records the item
    /// that was playing as a play. With the context spent and Repeat All on, the context starts
    /// over from its first row, with no copy; a shuffle that had nothing to play shuffles that
    /// lap (the whole context becomes its base). With nothing left, nothing plays. Repeat One is
    /// the player's to apply.
    fn next(&mut self) {
        let plays: Vec<Span> = self.current.iter().map(play_span).collect();
        if self.manual.is_empty() && self.cursor >= self.order.len() && self.repeat == Repeat::All {
            self.cursor = 0;
            self.shuffle_next_lap();
        }
        self.current = if !self.manual.is_empty() {
            let item = self.item_at(&self.manual, 0);
            self.manual.remove(0..1);
            item
        } else if self.cursor < self.order.len() {
            let item = self.item_at(&self.order, self.cursor);
            self.cursor += 1;
            item
        } else {
            None
        };
        if self.record_plays(plays) {
            self.clean_up();
        }
    }

    /// The Up Next indices of the named items that are in Up Next, ascending and distinct.
    /// Each item is found as [`QueueState::find_item`] would, but all in one sweep over each
    /// span list: O((s + k) log k) for k items.
    fn resolve(&self, items: &[ItemId]) -> Vec<u64> {
        let mut sorted: Vec<(&str, u64)> =
            items.iter().map(|item| (item.key.as_str(), item.pos)).collect();
        sorted.sort_unstable();
        sorted.dedup();
        let manual = self.manual.find_sorted(&sorted);
        let order = self.order_rows_of(&sorted);
        let mut at: Vec<u64> = manual
            .into_iter()
            .zip(order)
            .filter_map(|(manual, order)| {
                manual.or_else(|| {
                    let rest = order?.checked_sub(self.cursor)?;
                    Some(self.manual.len() + rest)
                })
            })
            .collect();
        at.sort_unstable();
        at.dedup();
        at
    }

    /// Takes the rows at Up Next indices `at` (ascending, distinct) out of the queue and
    /// returns them in queue order, one span per row until they are put back. A Manual Queue
    /// row that also has a passed place in the context loses it unless `keep_passed`. One pass
    /// over each span list, and one sweep to find those passed places, so a remove or move of
    /// k rows is O((s + k) log k).
    fn take(&mut self, at: &[u64], keep_passed: bool) -> Vec<Span> {
        let manual_len = self.manual.len();
        let (manual, rest) = at.split_at(at.partition_point(|&index| index < manual_len));
        let mut rows = if keep_passed { Vec::new() } else { self.passed_rows(manual) };
        // Passed places are behind the cursor, so they sort ahead of the Up Next rows.
        rows.sort_unstable();
        let passed = rows.len();
        rows.extend(rest.iter().map(|&index| self.cursor + index - manual_len));
        let mut taken = self.manual.remove_rows(manual);
        taken.extend(self.order.remove_rows(&rows).into_iter().skip(passed));
        self.cursor -= passed as u64;
        taken
    }

    /// The context rows of the passed places that the Manual Queue rows at `manual` have, in
    /// no particular order. One sweep, as [`QueueState::order_rows_of`].
    fn passed_rows(&self, manual: &[u64]) -> Vec<u64> {
        let items: Vec<ItemId> =
            manual.iter().filter_map(|&index| self.item_at(&self.manual, index)).collect();
        let mut sorted: Vec<(&str, u64)> =
            items.iter().map(|item| (item.key.as_str(), item.pos)).collect();
        sorted.sort_unstable();
        let found = self.order_rows_of(&sorted).into_iter().flatten();
        found.filter(|&row| row < self.cursor).collect()
    }

    /// Splices `spans` in at `spot`.
    fn put(&mut self, spot: Spot, spans: Vec<Span>) {
        match spot {
            Spot::Manual(index) => self.manual.splice(index, spans),
            Spot::Order(index) => self.order.splice(index, spans),
        }
    }

    /// Where `anchor` points now. The Up Next indices `moving` are about to leave, so none of
    /// them can anchor.
    fn spot(&self, anchor: &Anchor, moving: &[u64]) -> Result<Spot, QueueError> {
        Ok(match anchor {
            Anchor::ManualStart => Spot::Manual(0),
            Anchor::ManualEnd => Spot::Manual(self.manual.len()),
            Anchor::ContextEnd => Spot::Order(self.order.len()),
            Anchor::Before { item } => {
                let index = self.find_item(item).ok_or(QueueError::AnchorGone)?;
                if moving.binary_search(&index).is_ok() {
                    return Err(QueueError::AnchorGone);
                }
                match index.checked_sub(self.manual.len()) {
                    None => Spot::Manual(index),
                    Some(rest) => Spot::Order(self.cursor + rest),
                }
            }
        })
    }
}
