//! What the plugin keeps in its `state` between runs, and what it decides from it, without any
//! I/O: where the pass through the library stopped, and when each track it looked up but wrote
//! nothing for may be looked up again. Tested natively.

use std::time::Duration;

/// How long a track that a lookup wrote nothing for waits before it is looked up again, by
/// when MusicBrainz may have more.
pub const RETRY_AFTER_S: u64 = 30 * 24 * 60 * 60;

/// How many searches a lookup sends again after MusicBrainz turned one away as too many.
pub const RETRIES: u32 = 1;

/// How long into a run it stops starting lookups, in the pass or for a change. A lookup is at
/// most three requests, two searches and [`RETRIES`], each waiting up to 60 s for its turn and
/// 15 s for a reply, so a run that starts one just before this still ends inside its five
/// minutes (`design/plugins.md` §7).
pub const TIME_PER_RUN: Duration = Duration::from_secs(60);

/// The most tracks with no artist one run checks for a recorded miss. Each check is a host call,
/// which costs a tick of the run's compute budget whatever the time, so the time limit alone
/// would not bound a run through many misses that are not due yet.
pub const CHECKS_PER_RUN: u32 = 10_000;

/// The key the pass keeps its place under: the ID of the last track it finished with.
pub const CURSOR: &str = "cursor";

/// The key whether the last run could write is kept under, with when that last changed.
pub const MODE: &str = "mode";

/// What looking a track up came to, for what the plugin remembers of it.
#[derive(Debug, PartialEq)]
pub enum Seen {
    /// Nothing was written: MusicBrainz had no match, or more than one, or replacing files was
    /// not granted. The track waits [`RETRY_AFTER_S`].
    Missed,
    /// Nothing more to do: it has an artist, or its tags were written.
    Settled,
    /// The lookup or the write failed, so what was remembered of it stands, and the pass tries
    /// again on its next way round.
    Failed,
}

/// Whether the pass looks up a track with no artist at `now`, given when it was last missed and
/// when whether runs could write last changed: one never missed, one missed before that changed
/// or in the same second (what it found might be written now), or one whose wait is over.
pub fn is_due(missed_at: Option<u64>, now: u64, mode_since: u64) -> bool {
    missed_at.is_none_or(|at| at <= mode_since || at.saturating_add(RETRY_AFTER_S) <= now)
}

/// When whether runs could write last changed, given what was kept from the last run with when
/// it began, and whether this one can: unchanged keeps its time (but never one past `now`, which
/// a clock that was set ahead leaves), a change starts at `now`, and none kept counts as changed
/// at 0, so earlier misses stand.
pub fn mode_since(kept: Option<(bool, u64)>, can_write: bool, now: u64) -> u64 {
    match kept {
        Some((could, since)) if could == can_write => since.min(now),
        Some(_) => now,
        None => 0,
    }
}

/// Whether a run could write and when that began, as stored under [`MODE`].
pub fn encode_mode(can_write: bool, since: u64) -> Vec<u8> {
    let mut bytes = encode(since);
    bytes.push(u8::from(can_write));
    bytes
}

/// What [`encode_mode`] stored, or none if the bytes are not that.
pub fn decode_mode(bytes: &[u8]) -> Option<(bool, u64)> {
    let (since, [can_write]) = bytes.split_first_chunk::<8>()? else { return None };
    Some((*can_write != 0, u64::from_le_bytes(*since)))
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

#[cfg(test)]
mod tests;
