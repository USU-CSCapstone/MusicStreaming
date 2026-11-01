//! The queue's state and how a start builds it (`design/queue.md` §3, §5).

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use super::spans::{Recipe, Span, SpanList, View};

/// An item: position `pos` of the source or copy keyed `key`, written `<key>.<pos>`.
///
/// It names a queued row for as long as the row is queued.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemId {
    /// The source's or copy's key.
    pub key: String,
    /// The position in that view.
    pub pos: u64,
}

impl fmt::Display for ItemId {
    /// Writes `<key>.<pos>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.key, self.pos)
    }
}

impl FromStr for ItemId {
    type Err = String;

    /// Parses `<key>.<pos>`, splitting at the last dot.
    fn from_str(s: &str) -> Result<Self, String> {
        let (key, pos) = s.rsplit_once('.').ok_or_else(|| format!("item `{s}` has no position"))?;
        let pos = pos.parse().map_err(|_| format!("item `{s}` has a bad position"))?;
        Ok(Self { key: key.to_string(), pos })
    }
}

impl Serialize for ItemId {
    /// Serializes as the `<key>.<pos>` string.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ItemId {
    /// Reads the `<key>.<pos>` string, failing on a malformed one.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?.parse().map_err(de::Error::custom)
    }
}

/// A frozen list as a queue holds it: its length always, its hash once the server confirms it,
/// and its track IDs once attached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// The device-minted key, unique across sources and copies.
    pub key: String,
    /// The library every track belongs to.
    pub library_id: String,
    /// Lowercase hex SHA-256 of the track IDs. Opaque here; `None` until the server confirms.
    #[serde(rename = "list", default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The number of tracks, and so of positions.
    pub length: u64,
    /// How the list was made, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<Recipe>,
    /// The track IDs once attached. Never serialized: queue data carries no tracks.
    #[serde(skip)]
    pub(super) tracks: Option<Arc<[u64]>>,
}

impl Source {
    /// A source of `length` positions whose list hasn't arrived.
    pub fn new(
        key: impl Into<String>,
        library_id: impl Into<String>,
        length: u64,
        recipe: Option<Recipe>,
    ) -> Self {
        Self {
            key: key.into(),
            library_id: library_id.into(),
            hash: None,
            length,
            recipe,
            tracks: None,
        }
    }

    /// The attached track IDs, or `None` before the list arrives.
    pub fn tracks(&self) -> Option<&[u64]> {
        self.tracks.as_deref()
    }
}

/// A copy of some spans under a new identity: its position `p` is item `<key>.<p>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueCopy {
    /// The device-minted key, unique across sources and copies.
    pub key: String,
    /// The spans it covers, flattened: they name sources only.
    pub of: SpanList,
}

/// A run of consecutive plays of consecutive positions of one view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// The number of the run's first play.
    pub first: u64,
    /// The positions played.
    pub span: Span,
}

/// The context's one shuffle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shuffle {
    /// Fixes the permutation.
    pub seed: u32,
    /// The unplayed spans frozen when the shuffle began.
    pub base: SpanList,
}

/// What happens when the context ends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    /// Stop.
    #[default]
    Off,
    /// Start the context over.
    All,
    /// Play the current item again.
    One,
}

/// Why an operation on the queue was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    /// The key already names a source or copy.
    DuplicateKey,
    /// The position isn't in the source.
    OutOfRange,
    /// No source has the key.
    UnknownSource,
    /// The track list's length isn't the source's.
    LengthMismatch,
    /// The anchor names an item that isn't in Up Next, or one the same edit moves.
    AnchorGone,
    /// The play isn't in History, or played another item.
    PlayGone,
}

impl fmt::Display for QueueError {
    /// Writes a plain-language reason for the refusal.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DuplicateKey => "the key already names a source or copy",
            Self::OutOfRange => "the position is not in the source",
            Self::UnknownSource => "no source has that key",
            Self::LengthMismatch => "the track list's length is not the source's",
            Self::AnchorGone => "the item to insert before is no longer in Up Next",
            Self::PlayGone => "the play is no longer in History",
        })
    }
}

impl std::error::Error for QueueError {}

/// An account's queue: a few spans over views of frozen lists. This is the snapshot's `state`.
///
/// History and shuffle are declared so the JSON shape is whole; nothing sets them yet, and
/// reads don't go through a shuffle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueState {
    /// The lists the queue plays.
    pub sources: Vec<Source>,
    /// Copies of spans, made by edits.
    pub copies: Vec<QueueCopy>,
    /// The Manual Queue, in play order.
    pub manual: SpanList,
    /// The context, the passed part included.
    pub order: SpanList,
    /// How many rows of `order` are passed. The current context item is the last of them.
    pub cursor: u64,
    /// The item playing.
    pub current: Option<ItemId>,
    /// Play runs, oldest first.
    pub history: Vec<Run>,
    /// The number of the next play. Only counts up.
    pub next_play: u64,
    /// The context's shuffle, while shuffled.
    pub shuffle: Option<Shuffle>,
    /// What happens when the context ends.
    pub repeat: Repeat,
    /// Whether anything was queued, removed or moved; decides whether a start keeps the queue.
    pub adjusted: bool,
}

impl QueueState {
    /// Whether `key` names a source or a copy.
    pub fn has_key(&self, key: &str) -> bool {
        self.source(key).is_some() || self.copies.iter().any(|c| c.key == key)
    }

    /// The source keyed `key`.
    pub(super) fn source(&self, key: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.key == key)
    }

    /// Starts a new queue over `source`, replacing this one, with position `at` current and
    /// the Manual Queue empty. Only the repeat mode carries over (requirements/queue.md §5).
    ///
    /// Refused, changing nothing, if `source.key` already names a source or copy of the queue
    /// being replaced (so a retried start never replaces anything), or if `at` isn't a position.
    /// The caller checks that keys are UUIDs.
    pub fn start(&mut self, source: Source, at: u64) -> Result<(), QueueError> {
        if self.has_key(&source.key) {
            return Err(QueueError::DuplicateKey);
        }
        if at >= source.length {
            return Err(QueueError::OutOfRange);
        }
        *self = Self {
            order: SpanList::from_spans([Span::new(
                View::Key(source.key.clone()),
                0,
                source.length,
            )]),
            cursor: at + 1,
            current: Some(ItemId { key: source.key.clone(), pos: at }),
            sources: vec![source],
            repeat: self.repeat,
            ..Self::default()
        };
        Ok(())
    }

    /// Attaches the track IDs of source `key`, and its confirmed `hash`.
    ///
    /// Refused if no source has the key or `tracks` isn't as long as the source.
    pub fn attach_list(
        &mut self,
        key: &str,
        hash: String,
        tracks: Arc<[u64]>,
    ) -> Result<(), QueueError> {
        let source =
            self.sources.iter_mut().find(|s| s.key == key).ok_or(QueueError::UnknownSource)?;
        if tracks.len() as u64 != source.length {
            return Err(QueueError::LengthMismatch);
        }
        source.hash = Some(hash);
        source.tracks = Some(tracks);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn start_with_duplicate_key_is_refused() {
        let mut state = QueueState::default();
        state.start(Source::new("k", "lib", 3, None), 0).unwrap();
        let before = state.clone();
        assert_eq!(state.start(Source::new("k", "lib", 9, None), 0), Err(QueueError::DuplicateKey));
        state.copies.push(QueueCopy { key: "c".into(), of: SpanList::new() });
        let before_copy = state.clone();
        assert_eq!(state.start(Source::new("c", "lib", 9, None), 0), Err(QueueError::DuplicateKey));
        assert_eq!(state, before_copy);
        assert_ne!(state, before);
        assert_eq!(state.start(Source::new("n", "lib", 3, None), 3), Err(QueueError::OutOfRange));
        assert_eq!(state, before_copy);

        // A fresh key replaces the queue and nothing carries over.
        state.start(Source::new("n", "lib", 3, None), 2).unwrap();
        assert_eq!(state.sources.len(), 1);
        assert!(state.copies.is_empty() && state.manual.is_empty());
        assert_eq!((state.cursor, state.up_next_len()), (3, 0));
        crate::queue::tests::assert_invariants(&state);
    }

    #[test]
    fn start_keeps_repeat() {
        let mut state = QueueState::default();
        state.start(Source::new("a", "lib", 3, None), 0).unwrap();
        state.repeat = Repeat::One;
        state.start(Source::new("b", "lib", 2, None), 1).unwrap();
        assert_eq!(state.repeat, Repeat::One);
    }

    #[test]
    fn state_json_round_trip() {
        let mut state = QueueState::default();
        let mut source = Source::new(
            "k",
            "lib",
            4,
            Some(Recipe::Album { album_id: "7".into(), sort: "disc".into() }),
        );
        source.hash = Some("ab".into());
        state.start(source, 1).unwrap();
        state.sources.push(Source::new("s", "lib", 2, Some(Recipe::Tracks)));
        state.copies.push(QueueCopy {
            key: "c".into(),
            of: SpanList::from_spans([Span::new(View::Key("s".into()), 0, 2)]),
        });
        state.manual = SpanList::from_spans([
            Span::new(View::Key("c".into()), 0, 1),
            Span::new(View::Shuffle, 3, 2),
        ]);
        state.history.push(Run { first: 4, span: Span::new(View::Key("k".into()), 0, 1) });
        state.next_play = 5;
        state.shuffle = Some(Shuffle {
            seed: 9,
            base: SpanList::from_spans([Span::new(View::Key("k".into()), 2, 2)]),
        });
        state.repeat = Repeat::All;
        state.adjusted = true;
        state.attach_list("s", "cd".into(), Arc::from([1, 2])).unwrap();

        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(
            value,
            json!({
                "sources": [
                    {"key": "k", "libraryId": "lib", "list": "ab", "length": 4,
                     "recipe": {"type": "album", "albumId": "7", "sort": "disc"}},
                    {"key": "s", "libraryId": "lib", "list": "cd", "length": 2,
                     "recipe": {"type": "tracks"}},
                ],
                "copies": [{"key": "c", "of": [{"view": {"key": "s"}, "start": 0, "len": 2}]}],
                "manual": [
                    {"view": {"key": "c"}, "start": 0, "len": 1},
                    {"view": "shuffle", "start": 3, "len": 2},
                ],
                "order": [{"view": {"key": "k"}, "start": 0, "len": 4}],
                "cursor": 2,
                "current": "k.1",
                "history": [{"first": 4, "span": {"view": {"key": "k"}, "start": 0, "len": 1}}],
                "nextPlay": 5,
                "shuffle": {"seed": 9, "base": [{"view": {"key": "k"}, "start": 2, "len": 2}]},
                "repeat": "all",
                "adjusted": true,
            })
        );

        // The tracks stay out, and the rest comes back, running lengths included.
        let back: QueueState = serde_json::from_value(value).unwrap();
        state.sources[1].tracks = None;
        assert_eq!(back, state);
        assert_eq!(back.manual.len(), 3);
    }
}
