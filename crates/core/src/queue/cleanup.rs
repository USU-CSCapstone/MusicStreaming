//! Eager clean-up of sources and copies nothing refers to (`design/queue.md` §5).

use std::collections::HashSet;

use super::spans::{SpanList, View};
use super::state::QueueState;

/// Adds the keys `list` reads to `keys`.
fn add_keys<'a>(list: &'a SpanList, keys: &mut HashSet<&'a str>) {
    for span in list.spans() {
        if let View::Key(key) = &span.view {
            keys.insert(key);
        }
    }
}

impl QueueState {
    /// Drops every copy and source nothing refers to: `current`, or a span in the Manual Queue,
    /// the context, the shuffle's base, a copy or a History run. One pass over the entries; call
    /// it after an edit that removed rows. Nothing is renumbered.
    pub(super) fn clean_up(&mut self) {
        let mut used: HashSet<&str> = HashSet::new();
        used.extend(self.current.iter().map(|item| item.key.as_str()));
        add_keys(&self.manual, &mut used);
        add_keys(&self.order, &mut used);
        if let Some(shuffle) = &self.shuffle {
            add_keys(&shuffle.base, &mut used);
        }
        used.extend(self.history.iter().filter_map(|run| match &run.span.view {
            View::Key(key) => Some(key.as_str()),
            View::Shuffle => None,
        }));
        self.copies.retain(|copy| used.contains(copy.key.as_str()));

        // A surviving copy keeps the sources it names.
        for copy in &self.copies {
            add_keys(&copy.of, &mut used);
        }
        self.sources.retain(|source| used.contains(source.key.as_str()));
    }
}
