//! What the plugin keeps in its `state` between runs, and what it decides from it, without any
//! I/O: where the pass through the library stopped, and when each track it looked up but saved
//! nothing for may be looked up again. Tested natively.

use std::time::Duration;

use crate::lrclib::Found;

/// How long a track that a lookup saved nothing for waits before it is looked up again, unless
/// what kept it from being saved changes first.
pub const RETRY_AFTER_S: u64 = 30 * 24 * 60 * 60;

/// How long into a run the pass stops starting lookups. A lookup is at most two requests, each
/// waiting up to 60 s for its turn and 15 s for a reply, so a run that starts one just before
/// this still ends inside its five minutes (`design/plugins.md` §7).
pub const TIME_PER_RUN: Duration = Duration::from_secs(120);

/// The most tracks without lyrics one run checks for a recorded miss. Each check is a host call,
/// which costs a tick of the run's compute budget whatever the time, so the time limit alone
/// would not bound a run through many misses that are not due yet.
pub const CHECKS_PER_RUN: u32 = 10_000;

/// The key the pass keeps its place under: the ID of the last track it finished with.
pub const CURSOR: &str = "cursor";

/// What this run saves: whether it was granted adding files, and whether only synced lyrics
/// are wanted.
#[derive(Clone, Copy)]
pub struct Saving {
    pub can_write: bool,
    pub synced_only: bool,
}

/// Why a lookup saved nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Why {
    /// LRCLIB had no lyrics: nothing found, or an instrumental.
    Nothing,
    /// It found lyrics, but adding files was not granted.
    Unsaved,
    /// It found only plain lyrics, and only synced ones are wanted.
    PlainOnly,
}

/// What looking a track up came to, for what the plugin remembers of it.
#[derive(Debug, PartialEq)]
pub enum Seen {
    /// Nothing was saved, for this reason.
    Missed(Why),
    /// Nothing more to look for: it has lyrics, or they were saved.
    Settled,
    /// The lookup or the save failed, so what was remembered of it stands, and the pass tries
    /// again on its next way round.
    Failed,
}

/// What a lookup that answered `found` comes to: settled only when it has lyrics to save, and
/// otherwise a miss that says why.
pub fn seen(found: &Found, saving: Saving) -> Seen {
    match found {
        Found::Plain(_) if saving.synced_only => Seen::Missed(Why::PlainOnly),
        Found::Synced(_) | Found::Plain(_) if saving.can_write => Seen::Settled,
        Found::Synced(_) | Found::Plain(_) => Seen::Missed(Why::Unsaved),
        Found::Instrumental | Found::Nothing => Seen::Missed(Why::Nothing),
    }
}

/// A track a lookup saved nothing for: when it may be looked up again, in seconds since the
/// Unix epoch, and why.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Miss {
    pub retry_at: u64,
    pub why: Why,
}

impl Miss {
    /// A miss at `now`, for `why`: looked up again after [`RETRY_AFTER_S`].
    pub fn new(now: u64, why: Why) -> Miss {
        Miss { retry_at: now + RETRY_AFTER_S, why }
    }

    /// Whether this run looks the track up again at `now`: once it is due, or as soon as what
    /// kept it from being saved has changed.
    pub fn due(self, now: u64, saving: Saving) -> bool {
        self.retry_at <= now
            || match self.why {
                Why::Nothing => false,
                Why::Unsaved => saving.can_write,
                Why::PlainOnly => !saving.synced_only,
            }
    }

    /// The miss as it is stored: [`encode`]d `retry_at`, then a byte for why.
    pub fn encode(self) -> Vec<u8> {
        let why = match self.why {
            Why::Nothing => 0,
            Why::Unsaved => 1,
            Why::PlainOnly => 2,
        };
        let mut bytes = encode(self.retry_at);
        bytes.push(why);
        bytes
    }

    /// A miss [`Miss::encode`] stored.
    pub fn decode(bytes: &[u8]) -> Result<Miss, String> {
        let (why, retry_at) = bytes.split_last().ok_or("a stored miss is empty")?;
        let why = match why {
            0 => Why::Nothing,
            1 => Why::Unsaved,
            2 => Why::PlainOnly,
            other => return Err(format!("a stored miss has no reason {other}")),
        };
        Ok(Miss { retry_at: decode(retry_at)?, why })
    }
}

/// Whether the pass looks up a track without lyrics at `now`: one never missed, or a miss that
/// is [`Miss::due`].
pub fn wanted(miss: Option<Miss>, now: u64, saving: Saving) -> bool {
    miss.is_none_or(|m| m.due(now, saving))
}

/// The key a miss on track `id` is kept under.
pub fn miss_key(id: u64) -> String {
    format!("miss/{id}")
}

/// A track ID or a time, as it is stored.
pub fn encode(n: u64) -> Vec<u8> {
    n.to_le_bytes().to_vec()
}

/// A number [`encode`] stored.
pub fn decode(bytes: &[u8]) -> Result<u64, String> {
    <[u8; 8]>::try_from(bytes)
        .map(u64::from_le_bytes)
        .map_err(|_| format!("a stored number is {} bytes, not 8", bytes.len()))
}

#[cfg(test)]
mod tests;
