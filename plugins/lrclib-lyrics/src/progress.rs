//! What the plugin keeps in its `state` between runs, and what it decides from it, without any
//! I/O: where the pass through the library stopped, and when each track it looked up but saved
//! nothing for may be looked up again. Tested natively.

use std::time::Duration;

use crate::lrclib::Found;

/// How long a track that a lookup saved nothing for waits before it is looked up again.
pub const RETRY_AFTER_S: u64 = 30 * 24 * 60 * 60;

/// How long into a run it stops starting lookups, in the pass or for a change. A lookup is at
/// most two requests, each waiting up to 60 s for its turn and 15 s for a reply, so a run that
/// starts one just before this still ends inside its five minutes (`design/plugins.md` §7).
pub const TIME_PER_RUN: Duration = Duration::from_secs(120);

/// The most tracks without lyrics one run checks for a recorded miss. Each check is a host call,
/// which costs a tick of the run's compute budget whatever the time, so the time limit alone
/// would not bound a run through many misses that are not due yet.
pub const CHECKS_PER_RUN: u32 = 10_000;

/// The key the pass keeps its place under: the ID of the last track it finished with.
pub const CURSOR: &str = "cursor";

/// The key the tracks a change had no time for are kept under, for the pass to look up first.
pub const NEXT: &str = "next";

/// The most tracks kept under [`NEXT`]. Any past it are still found by the pass in its turn.
pub const MAX_NEXT: usize = 1000;

/// What this run saves: whether it was granted adding files, and whether only synced lyrics
/// are wanted.
#[derive(Clone, Copy)]
pub struct Saving {
    pub can_write: bool,
    pub synced_only: bool,
}

/// What looking a track up came to, for what the plugin remembers of it.
#[derive(Debug, PartialEq)]
pub enum Seen {
    /// Nothing was saved: LRCLIB had nothing, only plain lyrics when synced ones are wanted, or
    /// adding files was not granted. The track waits [`RETRY_AFTER_S`].
    Missed,
    /// Nothing more to look for: it has lyrics, or they were saved.
    Settled,
    /// The lookup or the save failed, so what was remembered of it stands, and the pass tries
    /// again on its next way round.
    Failed,
}

/// What a lookup that answered `found` comes to: settled only when it has lyrics to save.
pub fn seen(found: &Found, saving: Saving) -> Seen {
    match found {
        Found::Plain(_) if saving.synced_only => Seen::Missed,
        Found::Synced(_) | Found::Plain(_) if saving.can_write => Seen::Settled,
        _ => Seen::Missed,
    }
}

/// Whether the pass looks up a track without lyrics at `now`, given when it was last missed:
/// one never missed, or one whose wait is over.
pub fn is_due(missed_at: Option<u64>, now: u64) -> bool {
    missed_at.is_none_or(|at| at.saturating_add(RETRY_AFTER_S) <= now)
}

/// The key a miss on track `id` is kept under.
pub fn miss_key(id: u64) -> String {
    format!("miss/{id}")
}

/// A track ID or the time of a miss, as it is stored.
pub fn encode(n: u64) -> Vec<u8> {
    n.to_le_bytes().to_vec()
}

/// A number [`encode`] stored.
pub fn decode(bytes: &[u8]) -> Result<u64, String> {
    <[u8; 8]>::try_from(bytes)
        .map(u64::from_le_bytes)
        .map_err(|_| format!("a stored number is {} bytes, not 8", bytes.len()))
}

/// Track IDs, as they are stored: each [`encode`]d, one after another.
pub fn encode_ids(ids: &[u64]) -> Vec<u8> {
    ids.iter().flat_map(|id| id.to_le_bytes()).collect()
}

/// Track IDs [`encode_ids`] stored.
pub fn decode_ids(bytes: &[u8]) -> Result<Vec<u64>, String> {
    let (ids, []) = bytes.as_chunks::<8>() else {
        return Err(format!("stored track IDs are {} bytes, not a multiple of 8", bytes.len()));
    };
    Ok(ids.iter().copied().map(u64::from_le_bytes).collect())
}

#[cfg(test)]
mod tests;
