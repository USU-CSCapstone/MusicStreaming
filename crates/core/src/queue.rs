//! The queue model and its edits (`requirements/queue.md`, `design/general.md` §3).
//!
//! A queue is built from frozen **sources**: lists of tracks resolved once, never rewritten. Every
//! track in a source is an **item** with the ID `<sourceKey>.<index>`. The caller that adds a
//! source picks its key, so IDs never change when edits are replayed in another order. Duplicates
//! of one track are distinct items (§8).
//!
//! The queue stores items as `u32` positions in the sources laid end to end, so ordering 500,000
//! tracks costs 4 bytes each. It keeps four lists:
//!
//! - `manual`: the Manual Queue, which plays first.
//! - `order` and `cursor`: the context. Items before the cursor have been started or passed; the
//!   rest is the context's Up Next.
//! - `current`: what plays now.
//! - `history`: what has played, oldest first.
//!
//! Up Next is `manual` followed by the context's Up Next. An item is in at most one of Up Next,
//! History and `current`. Played context items also stay in `order` before the cursor, so a later
//! step can restart or unshuffle the context.
//!
//! Pure: no I/O, clock or randomness.

mod edit;
#[cfg(test)]
mod tests;

pub use edit::{Anchor, Edit, Error};
use serde::{Deserialize, Serialize};

/// A list of tracks resolved once. Never changes after it joins a queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// Chosen by whoever adds the source; unique within a queue.
    pub key: String,
    /// The library every track belongs to.
    pub library_id: String,
    /// Track IDs in the order they were resolved. A track may appear more than once.
    pub tracks: Vec<String>,
}

/// The track an item plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackRef<'a> {
    /// The library the track is in.
    pub library_id: &'a str,
    /// The track's ID.
    pub track_id: &'a str,
}

/// A queue. Edits go through [`Queue::apply`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Queue {
    sources: Vec<Source>,
    manual: Vec<u32>,
    order: Vec<u32>,
    cursor: usize,
    current: Option<u32>,
    history: Vec<u32>,
}

/// Where an insert lands, as a position in `manual` or in `order`.
enum Spot {
    Manual(usize),
    Context(usize),
}

impl Queue {
    /// Start a queue that plays `source` from the item at `start`. Earlier items count as passed:
    /// they are not in Up Next or History. Fails if `start` is past the end.
    pub fn start(source: Source, start: usize) -> Result<Self, Error> {
        if start >= source.tracks.len() {
            return Err(Error::StartOutOfRange);
        }
        let count = to_item(source.tracks.len())?;
        Ok(Self {
            order: (0..count).collect(),
            cursor: start + 1,
            current: Some(to_item(start)?),
            sources: vec![source],
            ..Self::default()
        })
    }

    /// The ID of the item playing now, if any.
    pub fn current(&self) -> Option<String> {
        self.current.and_then(|item| self.id_of(item))
    }

    /// The Manual Queue's item IDs, in play order.
    pub fn manual(&self) -> impl Iterator<Item = String> + '_ {
        self.ids(&self.manual)
    }

    /// The context's Up Next item IDs, in play order; they play after the Manual Queue.
    pub fn upcoming(&self) -> impl Iterator<Item = String> + '_ {
        self.ids(self.tail())
    }

    /// The History item IDs, oldest first.
    pub fn history(&self) -> impl Iterator<Item = String> + '_ {
        self.ids(&self.history)
    }

    /// The track an item ID plays, or `None` if the queue has no such item.
    pub fn track(&self, id: &str) -> Option<TrackRef<'_>> {
        let (source, index) = self.span(self.locate(id)?)?;
        Some(TrackRef { library_id: &source.library_id, track_id: source.tracks.get(index)? })
    }

    /// Apply one edit. An edit that fails leaves the queue unchanged.
    ///
    /// Items an edit names that are no longer where it expects are ignored. An anchor that is
    /// gone fails the whole edit, so the caller can revert and say so.
    pub fn apply(&mut self, edit: &Edit) -> Result<(), Error> {
        match edit {
            Edit::Add { source, before } => self.add(source, before),
            Edit::Move { items, before } => self.move_items(items, before),
            Edit::Remove { items } => {
                let named = self.resolve(items);
                self.take_named(&named);
                Ok(())
            }
            Edit::Jump { item } => {
                self.jump(item);
                Ok(())
            }
            Edit::Next => {
                self.next();
                Ok(())
            }
            Edit::Previous => {
                self.previous();
                Ok(())
            }
        }
    }

    /// Add a source's items at the anchor.
    fn add(&mut self, source: &Source, before: &Anchor) -> Result<(), Error> {
        let spot = self.spot(before, &[])?;
        if self.sources.iter().any(|known| known.key == source.key) {
            return Err(Error::DuplicateSource);
        }
        let count: usize = self.sources.iter().map(|s| s.tracks.len()).sum();
        let first = to_item(count)?;
        let end = to_item(count + source.tracks.len())?;
        self.sources.push(source.clone());
        self.insert(spot, (first..end).collect());
        Ok(())
    }

    /// Move the named items to the anchor.
    fn move_items(&mut self, ids: &[String], before: &Anchor) -> Result<(), Error> {
        let named = self.resolve(ids);
        // Check the anchor first so a failed move changes nothing; it is found again after the
        // items leave, because taking them shifts positions.
        self.spot(before, &named)?;
        let moved = self.take_named(&named);
        let spot = self.spot(before, &named)?;
        self.insert(spot, moved);
        Ok(())
    }

    /// Play an item now; see [`Edit::Jump`]. Does nothing if the item is not in Up Next or
    /// History.
    fn jump(&mut self, id: &str) {
        let Some(item) = self.locate(id) else { return };
        if let Some(at) = self.history.iter().position(|&i| i == item) {
            self.history.remove(at);
            self.retire_current();
        } else if let Some(at) = self.manual.iter().position(|&i| i == item) {
            self.retire_current();
            self.history.extend(self.manual.drain(..at));
            self.manual.remove(0);
        } else if let Some(at) = self.tail().iter().position(|&i| i == item) {
            let end = self.cursor + at;
            self.retire_current();
            self.history.append(&mut self.manual);
            self.history.extend_from_slice(&self.order[self.cursor..end]);
            self.cursor = end + 1;
        } else {
            return;
        }
        self.current = Some(item);
    }

    /// Play the first item of Up Next, and move what was playing to History.
    fn next(&mut self) {
        let upcoming = if self.manual.is_empty() {
            let first = self.tail().first().copied();
            if first.is_some() {
                self.cursor += 1;
            }
            first
        } else {
            Some(self.manual.remove(0))
        };
        self.retire_current();
        self.current = upcoming;
    }

    /// Play the most recent History item. What was playing goes back to the front of Up Next:
    /// in place in the context if it came from there, else the front of the Manual Queue.
    fn previous(&mut self) {
        let Some(item) = self.history.pop() else {
            return;
        };
        if let Some(playing) = self.current.take() {
            let in_place =
                self.cursor.checked_sub(1).filter(|&at| self.order.get(at) == Some(&playing));
            match in_place {
                Some(at) => self.cursor = at,
                None => {
                    self.forget_passed(&[playing]);
                    self.manual.insert(0, playing);
                }
            }
        }
        self.current = Some(item);
    }

    /// Move what is playing, if anything, to History.
    fn retire_current(&mut self) {
        if let Some(item) = self.current.take() {
            self.history.push(item);
        }
    }

    /// Take the named items (sorted, from [`Queue::resolve`]) out of History, the Manual Queue and
    /// the context's Up Next. Returns them in that order. Items taken from History are also
    /// forgotten as passed, so they can re-enter Up Next without appearing twice.
    fn take_named(&mut self, named: &[u32]) -> Vec<u32> {
        let mut taken = Vec::new();
        self.history.retain(|&item| keep_or_take(named, item, &mut taken));
        let mut from_history = taken.clone();
        from_history.sort_unstable();
        self.forget_passed(&from_history);

        self.manual.retain(|&item| keep_or_take(named, item, &mut taken));

        let cursor = self.cursor;
        let mut position = 0;
        self.order.retain(|&item| {
            position += 1;
            position <= cursor || keep_or_take(named, item, &mut taken)
        });
        taken
    }

    /// Remove the sorted `items` from the passed part of the context.
    fn forget_passed(&mut self, items: &[u32]) {
        if items.is_empty() {
            return;
        }
        let cursor = self.cursor;
        let mut position = 0;
        let mut forgotten = 0;
        self.order.retain(|&item| {
            position += 1;
            let forget = position <= cursor && has(items, item);
            forgotten += usize::from(forget);
            !forget
        });
        self.cursor -= forgotten;
    }

    /// Put items at a spot.
    fn insert(&mut self, spot: Spot, items: Vec<u32>) {
        let (list, at) = match spot {
            Spot::Manual(at) => (&mut self.manual, at),
            Spot::Context(at) => (&mut self.order, at),
        };
        list.splice(at..at, items);
    }

    /// Find where an anchor points right now. `moving` (sorted) are items about to leave Up Next,
    /// which cannot anchor their own move.
    fn spot(&self, anchor: &Anchor, moving: &[u32]) -> Result<Spot, Error> {
        match anchor {
            Anchor::ManualStart => Ok(Spot::Manual(0)),
            Anchor::ManualEnd => Ok(Spot::Manual(self.manual.len())),
            Anchor::ContextEnd => Ok(Spot::Context(self.order.len())),
            Anchor::Before { item } => {
                let item = self.locate(item).ok_or(Error::AnchorGone)?;
                if has(moving, item) {
                    return Err(Error::AnchorGone);
                }
                if let Some(at) = self.manual.iter().position(|&i| i == item) {
                    Ok(Spot::Manual(at))
                } else if let Some(at) = self.tail().iter().position(|&i| i == item) {
                    Ok(Spot::Context(self.cursor + at))
                } else {
                    Err(Error::AnchorGone)
                }
            }
        }
    }

    /// The items an edit names, sorted and without duplicates. IDs the queue does not know are left out.
    fn resolve(&self, ids: &[String]) -> Vec<u32> {
        let mut items: Vec<u32> = ids.iter().filter_map(|id| self.locate(id)).collect();
        items.sort_unstable();
        items.dedup();
        items
    }

    /// The context's Up Next.
    fn tail(&self) -> &[u32] {
        self.order.get(self.cursor..).unwrap_or_default()
    }

    /// Item IDs for a list of items, skipping any the sources do not hold.
    fn ids<'a>(&'a self, items: &'a [u32]) -> impl Iterator<Item = String> + 'a {
        items.iter().filter_map(|&item| self.id_of(item))
    }

    /// The ID of an item.
    fn id_of(&self, item: u32) -> Option<String> {
        let (source, index) = self.span(item)?;
        Some(format!("{}.{index}", source.key))
    }

    /// The item an ID names, or `None` if no source has it. Walks the sources, so cost grows with
    /// the number of sources, not tracks.
    fn locate(&self, id: &str) -> Option<u32> {
        let (key, index) = id.rsplit_once('.')?;
        let index = usize::try_from(index.parse::<u32>().ok()?).ok()?;
        let mut first = 0;
        for source in &self.sources {
            if source.key == key {
                return (index < source.tracks.len())
                    .then(|| to_item(first + index).ok())
                    .flatten();
            }
            first += source.tracks.len();
        }
        None
    }

    /// The source an item belongs to and its index in that source.
    fn span(&self, item: u32) -> Option<(&Source, usize)> {
        let mut rest = usize::try_from(item).ok()?;
        for source in &self.sources {
            if rest < source.tracks.len() {
                return Some((source, rest));
            }
            rest -= source.tracks.len();
        }
        None
    }
}

/// Convert a count or index to an item number.
fn to_item(n: usize) -> Result<u32, Error> {
    u32::try_from(n).map_err(|_| Error::TooManyItems)
}

/// Whether a sorted list holds an item.
fn has(sorted: &[u32], item: u32) -> bool {
    sorted.binary_search(&item).is_ok()
}

/// For `retain`: keep `item` unless it is named, in which case collect it into `taken`.
fn keep_or_take(named: &[u32], item: u32, taken: &mut Vec<u32>) -> bool {
    let named = has(named, item);
    if named {
        taken.push(item);
    }
    !named
}
