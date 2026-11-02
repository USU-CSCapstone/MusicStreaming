//! The edits that change a queue's Up Next (`design/queue.md` §5, §6).
//!
//! Edits name items, never positions, so they mean the same thing whatever order they arrive
//! in. They serialize as tagged JSON: they are the wire's `queueEdit` payloads.

use serde::{Deserialize, Serialize};

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

/// One change to a queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
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

/// The item a row of a span list reads, or `None` past the end or for a shuffle row.
fn item_of(row: Option<(&View, u64)>) -> Option<ItemId> {
    match row? {
        (View::Key(key), pos) => Some(ItemId { key: key.clone(), pos }),
        (View::Shuffle, _) => None,
    }
}

impl QueueState {
    /// Applies one edit. An edit that fails changes nothing.
    ///
    /// Items an edit names that aren't in Up Next are ignored, and an edit that then does
    /// nothing leaves the state as it was, `adjusted` included. An anchor that isn't in Up Next
    /// fails the whole edit with [`QueueError::AnchorGone`], so a device can revert and say so.
    /// `adjusted` is set by an add, remove or move that acts, and by no other edit.
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
        self.take(&at);
        self.adjusted = true;
        self.clean_up();
    }

    /// Moves the named items of Up Next to the anchor.
    fn move_items(&mut self, items: &[ItemId], before: &Anchor) -> Result<(), QueueError> {
        let at = self.resolve(items);
        self.spot(before, &at)?;
        if at.is_empty() {
            return Ok(());
        }
        let moved = self.take(&at);
        // Taking the rows shifted every position, so the anchor is found again.
        let spot = self.spot(before, &[])?;
        self.put(spot, moved);
        self.adjusted = true;
        Ok(())
    }

    /// Plays `item` now. Everything ahead of it in Up Next leaves it. Does nothing if `item`
    /// isn't in Up Next.
    fn jump(&mut self, item: &ItemId) {
        let Some(index) = self.find_item(item) else { return };
        match index.checked_sub(self.manual.len()) {
            None => {
                self.manual.remove(0..index + 1);
            }
            Some(rest) => {
                self.manual = Default::default();
                self.cursor += rest + 1;
            }
        }
        self.current = Some(item.clone());
        self.clean_up();
    }

    /// Plays the front of the Manual Queue, or else the next context row. With nothing left,
    /// nothing plays.
    fn next(&mut self) {
        self.current = if !self.manual.is_empty() {
            let item = item_of(self.manual.get(0));
            self.manual.remove(0..1);
            item
        } else if self.cursor < self.order.len() {
            let item = item_of(self.order.get(self.cursor));
            self.cursor += 1;
            item
        } else {
            None
        };
        self.clean_up();
    }

    /// The Up Next indices of the named items that are in Up Next, ascending and distinct.
    fn resolve(&self, items: &[ItemId]) -> Vec<u64> {
        let mut at: Vec<u64> = items.iter().filter_map(|item| self.find_item(item)).collect();
        at.sort_unstable();
        at.dedup();
        at
    }

    /// Takes the rows at Up Next indices `at` (ascending, distinct) out of the queue and
    /// returns them in queue order, one span per row until they are put back. One pass over
    /// each span list, so a remove or move of k rows is O(s + k).
    fn take(&mut self, at: &[u64]) -> Vec<Span> {
        let manual_len = self.manual.len();
        let (manual, rest) = at.split_at(at.partition_point(|&index| index < manual_len));
        let rows: Vec<u64> = rest.iter().map(|&index| self.cursor + index - manual_len).collect();
        let mut taken = self.manual.remove_rows(manual);
        taken.extend(self.order.remove_rows(&rows));
        taken
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
