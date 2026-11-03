//! Reading rows and finding items (`design/queue.md` §3, §6).

use super::spans::{SpanList, View};
use super::state::{ItemId, QueueState};

/// One row of the queue: which item it is, what it reads, and its track once the list is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row<'a> {
    /// The row's item.
    pub item: ItemId,
    /// The key of the source whose list the row reads (a copy's row reads its source).
    pub source: &'a str,
    /// The position in that source's list.
    pub position: u64,
    /// The track ID, or `None` while the source's list hasn't arrived.
    pub track: Option<u64>,
}

impl QueueState {
    /// The number of rows in Up Next: the Manual Queue, then the context past the cursor.
    pub fn up_next_len(&self) -> u64 {
        self.manual.len() + self.order.len().saturating_sub(self.cursor)
    }

    /// Up Next row `index`, or `None` past the end or where the row can't be read.
    pub fn up_next_row(&self, index: u64) -> Option<Row<'_>> {
        match index.checked_sub(self.manual.len()) {
            None => self.row_in(&self.manual, index),
            Some(rest) => self.row_in(&self.order, self.cursor.checked_add(rest)?),
        }
    }

    /// Context row `index`, the passed part included, or `None` past the end or where the row
    /// can't be read.
    pub fn context_row(&self, index: u64) -> Option<Row<'_>> {
        self.row_in(&self.order, index)
    }

    /// The row of the item playing, or `None` if nothing plays or the row can't be read.
    pub fn current_row(&self) -> Option<Row<'_>> {
        let item = self.current.as_ref()?;
        self.row_of(&item.key, item.pos)
    }

    /// The Up Next index of `item`, or `None` if it isn't in Up Next. The Manual Queue is
    /// searched first, and a context item counts only past the cursor. O(spans).
    pub fn find_item(&self, item: &ItemId) -> Option<u64> {
        let view = View::Key(item.key.clone());
        if let Some(i) = self.manual.find(&view, item.pos) {
            return Some(i);
        }
        let i = self.order.find(&view, item.pos)?;
        i.checked_sub(self.cursor).map(|rest| self.manual.len() + rest)
    }

    /// Row `index` of `spans`.
    fn row_in(&self, spans: &SpanList, index: u64) -> Option<Row<'_>> {
        match spans.get(index)? {
            (View::Key(key), pos) => self.row_of(key, pos),
            // Shuffle views can't be read yet.
            (View::Shuffle, _) => None,
        }
    }

    /// The row for position `pos` of the source or copy `key`: a copy's position reads the
    /// source position its spans name.
    fn row_of(&self, key: &str, pos: u64) -> Option<Row<'_>> {
        let (source, position) = match self.source(key) {
            Some(source) => (source, pos),
            None => {
                let copy = self.copies.iter().find(|c| c.key == key)?;
                match copy.of.get(pos)? {
                    (View::Key(k), p) => (self.source(k)?, p),
                    (View::Shuffle, _) => return None,
                }
            }
        };
        if position >= source.length {
            return None;
        }
        let track = source
            .tracks()
            .and_then(|t| usize::try_from(position).ok().and_then(|p| t.get(p)))
            .copied();
        Some(Row {
            item: ItemId { key: key.to_string(), pos },
            source: &source.key,
            position,
            track,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::spans::Span;
    use super::*;
    use crate::queue::state::{QueueCopy, Source};

    /// A queue started over a source `k` of `length` tracks at position `at`.
    fn started(length: u64, at: u64) -> QueueState {
        let mut state = QueueState::default();
        state.start(Source::new("k", "lib", length, None), at).unwrap();
        state
    }

    /// The item at position `pos` of `key`.
    fn item(key: &str, pos: u64) -> ItemId {
        ItemId { key: key.to_string(), pos }
    }

    #[test]
    fn rows_unknown_until_list_attaches() {
        let mut state = started(5, 1);
        assert_eq!(state.up_next_len(), 3);
        let row = state.up_next_row(0).unwrap();
        assert_eq!((row.item, row.source, row.position, row.track), (item("k", 2), "k", 2, None));
        assert_eq!(state.current_row().unwrap().track, None);
        assert!(state.up_next_row(3).is_none());
        assert!(state.up_next_row(u64::MAX).is_none());

        state.attach_list("k", "h".into(), Arc::from([10, 11, 12, 13, 14])).unwrap();
        assert_eq!(state.up_next_row(0).unwrap().track, Some(12));
        assert_eq!(state.up_next_row(2).unwrap().track, Some(14));
        assert_eq!(state.current_row().unwrap().track, Some(11));
        assert_eq!(state.context_row(0).unwrap().track, Some(10));
        assert_eq!(state.sources[0].hash.as_deref(), Some("h"));
    }

    #[test]
    fn attach_refuses_wrong_length_and_unknown_source() {
        let mut state = started(2, 0);
        assert_eq!(
            state.attach_list("k", "h".into(), Arc::from([1])),
            Err(crate::QueueError::LengthMismatch)
        );
        assert_eq!(
            state.attach_list("x", "h".into(), Arc::from([1, 2])),
            Err(crate::QueueError::UnknownSource)
        );
        assert!(state.sources[0].tracks().is_none());
    }

    #[test]
    fn find_item_in_up_next() {
        let mut state = started(5, 1);
        assert_eq!(state.find_item(&item("k", 3)), Some(1));
        assert_eq!(state.find_item(&item("k", 2)), Some(0));
        // The current item and passed ones are not in Up Next.
        assert_eq!(state.find_item(&item("k", 1)), None);
        assert_eq!(state.find_item(&item("k", 0)), None);
        assert_eq!(state.find_item(&item("x", 3)), None);

        // A copy in the Manual Queue sits ahead of the context and reads its source.
        state.sources.push(Source::new("s", "lib", 4, None));
        state.copies.push(QueueCopy {
            key: "c".into(),
            of: SpanList::from_spans([Span::new(View::Key("s".into()), 2, 2)]),
        });
        state.manual = SpanList::from_spans([Span::new(View::Key("c".into()), 0, 2)]);
        assert_eq!(state.up_next_len(), 5);
        assert_eq!(state.find_item(&item("c", 1)), Some(1));
        assert_eq!(state.find_item(&item("k", 3)), Some(3));
        let row = state.up_next_row(1).unwrap();
        assert_eq!((row.item, row.source, row.position), (item("c", 1), "s", 3));
        // The source's own item isn't the copy's.
        assert_eq!(state.find_item(&item("s", 3)), None);
    }

    #[test]
    fn find_item_across_several_spans() {
        let mut state = started(6, 0);
        state.sources.push(Source::new("s", "lib", 2, None));
        let span = |key: &str, start, len| Span::new(View::Key(key.into()), start, len);
        state.order = SpanList::from_spans([span("k", 0, 2), span("s", 0, 2), span("k", 3, 2)]);
        state.cursor = 1;
        assert_eq!(state.find_item(&item("k", 3)), Some(3));
        assert_eq!(state.find_item(&item("k", 4)), Some(4));
        assert_eq!(state.find_item(&item("s", 1)), Some(2));
        assert_eq!(state.find_item(&item("k", 1)), Some(0));
        // Just past a span's end, and a passed row.
        assert_eq!(state.find_item(&item("k", 2)), None);
        assert_eq!(state.find_item(&item("k", 0)), None);
    }
}
