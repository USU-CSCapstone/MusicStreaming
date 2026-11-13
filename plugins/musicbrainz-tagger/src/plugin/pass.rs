//! The pass through the library, a part per run, the tracks a change names, and what the plugin
//! remembers of each track it looked up, kept in its `state`.

use std::time::Instant;

use super::jewelcase::plugin::library::{self, Track};
use super::jewelcase::plugin::state;
use crate::progress::{self, CHECKS_PER_RUN, CURSOR, MODE, Seen};

/// Tracks read from the library at a time: the most the host gives.
const PAGE: u32 = 500;

/// How far a run of the pass got.
pub struct Walked {
    /// Whether it reached the end of the library, where the next run starts over.
    pub finished: bool,
    /// The tracks that had an artist, which it went by.
    pub tagged: u32,
}

/// Notes whether this run can write, and answers when that last changed: now if the last run's
/// differed, so misses from before are due again. Keeps it only when it differs, and reads it
/// once, since each state call costs a tick.
pub fn mode_since(can_write: bool, now: u64) -> Result<u64, String> {
    let kept = state::get(MODE)?.and_then(|bytes| progress::decode_mode(&bytes));
    let since = progress::mode_since(kept, can_write, now);
    if kept != Some((can_write, since)) {
        state::set(MODE, &progress::encode_mode(can_write, since))?;
    }
    Ok(since)
}

/// Goes on with the pass from where it last stopped, looking up with `look` each track with no
/// artist, except misses still waiting: those made since writing was last granted or taken away
/// (`mode_since`), less than [`progress::RETRY_AFTER_S`] ago. It stops starting lookups at
/// `deadline`, and stops after [`CHECKS_PER_RUN`] checks. Its place is kept after each lookup,
/// so a run cut short loses nothing it did. An error from `look` ends it before that track, so
/// the next run looks it up first.
pub fn resume(
    now: u64,
    mode_since: u64,
    deadline: Instant,
    mut look: impl FnMut(&Track) -> Result<Seen, String>,
) -> Result<Walked, String> {
    let mut walked = Walked { finished: false, tagged: 0 };
    let mut cursor = state::get(CURSOR)?.map(|bytes| progress::decode(&bytes)).transpose()?;
    let mut checks = 0;
    loop {
        let page = library::tracks(cursor, PAGE)?;
        let Some(last) = page.last() else { break };
        cursor = Some(last.id);
        for t in &page {
            if !t.artists.is_empty() {
                walked.tagged += 1;
                continue;
            }
            checks += 1;
            if progress::is_due(missed_at(t.id)?, now, mode_since) {
                note(t.id, look(t)?, now)?;
                state::set(CURSOR, &progress::encode(t.id))?;
                // Read only after a lookup: reading the clock is a host call, which costs a tick.
                if Instant::now() >= deadline {
                    return Ok(walked);
                }
            }
            if checks == CHECKS_PER_RUN {
                state::set(CURSOR, &progress::encode(t.id))?;
                return Ok(walked);
            }
        }
    }
    state::delete(CURSOR)?;
    walked.finished = true;
    Ok(walked)
}

/// Looks up with `look` each of the tracks `changed` names, and forgets any miss on those
/// `removed` names. At `deadline` it stops starting lookups, so a slow service cannot hold the
/// run past its time limit, and forgets any miss on the tracks with no artist it did not reach,
/// so the pass looks them up in its turn. Answers how many it left. An error from `look` ends
/// it, so the event is delivered again.
pub fn changed(
    changed: &[u64],
    removed: &[u64],
    now: u64,
    deadline: Instant,
    mut look: impl FnMut(&Track) -> Result<Seen, String>,
) -> Result<u32, String> {
    let mut left = 0;
    for ids in changed.chunks(PAGE as usize) {
        for t in library::get_tracks(ids)? {
            if Instant::now() < deadline {
                note(t.id, look(&t)?, now)?;
            } else if t.artists.is_empty() {
                state::delete(&progress::miss_key(t.id))?;
                left += 1;
            }
        }
    }
    for &id in removed {
        state::delete(&progress::miss_key(id))?;
    }
    Ok(left)
}

/// Remembers what looking up track `id` at `now` came to: a miss is kept with the time, and
/// anything else clears it.
fn note(id: u64, seen: Seen, now: u64) -> Result<(), String> {
    let key = progress::miss_key(id);
    match seen {
        Seen::Missed => state::set(&key, &progress::encode(now))?,
        Seen::Settled => state::delete(&key)?,
        Seen::Failed => {}
    }
    Ok(())
}

/// When track `id` was last missed, if it was. A stored value that is not a number counts as none.
fn missed_at(id: u64) -> Result<Option<u64>, String> {
    let kept = state::get(&progress::miss_key(id))?;
    Ok(kept.and_then(|bytes| progress::decode(&bytes).ok()))
}
