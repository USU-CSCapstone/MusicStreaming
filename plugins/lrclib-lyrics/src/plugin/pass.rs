//! The pass through the library, a part per run, and what the plugin remembers of each track it
//! looked up, both kept in its `state`.

use std::time::Instant;

use super::jewelcase::plugin::library::{self, Track};
use super::jewelcase::plugin::state;
use crate::progress::{self, CHECKS_PER_RUN, CURSOR, Miss, Saving, Seen, TIME_PER_RUN};

/// Tracks read from the library at a time: the most the host gives.
pub const PAGE: u32 = 500;

/// How far a run of the pass got.
pub struct Walked {
    /// Whether it reached the end of the library, where the next run starts over.
    pub finished: bool,
    /// The tracks with lyrics it went by.
    pub had_lyrics: u32,
}

/// Goes on with the pass from where it last stopped, looking up with `look` each track without
/// lyrics, except misses not [`Miss::due`] for a run that saves as `saving` says. It stops
/// starting lookups after
/// [`TIME_PER_RUN`], and stops after [`CHECKS_PER_RUN`] checks. Its place is kept after each
/// lookup, so a run cut short loses nothing it did.
pub fn resume(
    now: u64,
    saving: Saving,
    mut look: impl FnMut(&Track) -> Seen,
) -> Result<Walked, String> {
    let started = Instant::now();
    let mut walked = Walked { finished: false, had_lyrics: 0 };
    let mut cursor = state::get(CURSOR)?.map(|bytes| progress::decode(&bytes)).transpose()?;
    let mut checks = 0;
    loop {
        let page = library::tracks(cursor, PAGE)?;
        let Some(last) = page.last() else { break };
        cursor = Some(last.id);
        for t in &page {
            if t.has_lyrics {
                walked.had_lyrics += 1;
                continue;
            }
            checks += 1;
            if progress::wanted(miss(t.id)?, now, saving) {
                note(t.id, look(t), now)?;
                state::set(CURSOR, &progress::encode(t.id))?;
                // Read only after a lookup: reading the clock is a host call, which costs a tick.
                if started.elapsed() >= TIME_PER_RUN {
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

/// Remembers what looking up track `id` at `now` came to.
pub fn note(id: u64, seen: Seen, now: u64) -> Result<(), String> {
    match seen {
        Seen::Missed(why) => state::set(&progress::miss_key(id), &Miss::new(now, why).encode()),
        Seen::Settled => forget(id),
        Seen::Failed => Ok(()),
    }
}

/// Forgets any miss on track `id`, such as once it has lyrics or has left the library.
pub fn forget(id: u64) -> Result<(), String> {
    let key = progress::miss_key(id);
    match state::get(&key)? {
        Some(_) => state::delete(&key),
        None => Ok(()),
    }
}

/// The miss kept on track `id`, if there is one.
fn miss(id: u64) -> Result<Option<Miss>, String> {
    state::get(&progress::miss_key(id))?.map(|bytes| Miss::decode(&bytes)).transpose()
}
