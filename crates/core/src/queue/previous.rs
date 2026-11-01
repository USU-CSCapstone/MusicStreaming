//! Previous: step back one play (`design/queue.md` §5, §6).

use super::edit::item_of;
use super::history::play_span;
use super::spans::{SpanList, View};
use super::state::{ItemId, QueueCopy, QueueError, QueueState};

impl QueueState {
    /// Pops the last play, of item X, and makes it current. The item that was playing goes to
    /// the front of Up Next, so Up Next is exactly that item and then Up Next as it was: back
    /// into the context by moving the cursor back one when it is the context item just passed
    /// and the Manual Queue is empty, and otherwise to the front of the Manual Queue, where a
    /// context track keeps its passed place in `order`.
    ///
    /// If X is then in Up Next, a copy of its position under `key` is current instead and X
    /// stays queued; `key` is used only then, and a `key` already in use fails the edit, leaving
    /// the state as it was. With an empty History nothing happens, and the player restarts the
    /// track. Never sets `adjusted`.
    pub(super) fn previous(&mut self, key: &str) -> Result<(), QueueError> {
        let Some(last) = self.history.last() else { return Ok(()) };
        let View::Key(key_of_x) = &last.span.view else { return Ok(()) };
        let x = ItemId { key: key_of_x.clone(), pos: last.span.start + last.span.len - 1 };
        let old = self.current.clone();
        // The old current joins Up Next, and nothing else enters or leaves it.
        let copy_needed = old.as_ref() == Some(&x) || self.find_item(&x).is_some();
        if copy_needed && self.has_key(key) {
            return Err(QueueError::DuplicateKey);
        }

        self.pop_play();
        if let Some(old) = &old {
            let just_passed = self
                .cursor
                .checked_sub(1)
                .is_some_and(|row| item_of(self.order.get(row)).as_ref() == Some(old));
            if self.manual.is_empty() && just_passed {
                self.cursor -= 1;
            } else {
                self.manual.splice(0, [play_span(old)]);
            }
        }
        self.current = Some(if copy_needed {
            let of = SpanList::from_spans(self.source_spans(&x));
            self.copies.push(QueueCopy { key: key.to_string(), of });
            ItemId { key: key.to_string(), pos: 0 }
        } else {
            x
        });
        Ok(())
    }

    /// Takes the newest play out of History, leaving `nextPlay` alone so its number is never
    /// reused. It lets go of no view: the item it named becomes current or stays in Up Next.
    fn pop_play(&mut self) {
        let Some(run) = self.history.last_mut() else { return };
        run.span.len -= 1;
        if run.span.len == 0 {
            self.history.pop();
        }
    }
}
