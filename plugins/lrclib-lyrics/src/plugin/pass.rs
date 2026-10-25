//! The pass through the library, a part per run, the tracks a change names, and what the plugin
//! remembers of each track it looked up, kept in its `state`.

use std::time::Instant;

use super::jewelcase::plugin::library::{self, Track};
use super::jewelcase::plugin::state;
use crate::progress::{self, CHECKS_PER_RUN, CURSOR, MAX_NEXT, NEXT, Seen};

/// Tracks read from the library at a time: the most the host gives.
const PAGE: usize = 500;

/// How far a run of the pass got.
pub struct Walked {
    /// Whether it reached the end of the library, where the next run starts over.
    pub finished: bool,
    /// The tracks with lyrics it went by.
    pub had_lyrics: u32,
}

/// Goes on with the pass, looking up with `look` first the tracks a change left for it
/// ([`catch_up`]), then, from where it last stopped, each track without lyrics, except misses
/// still waiting. It stops starting lookups at `deadline`, and stops after [`CHECKS_PER_RUN`]
/// checks. Its place is kept after each lookup, so a run cut short loses nothing it did. An
/// error from `look` ends it before that track, so the next run looks it up first.
pub fn resume(
    now: u64,
    deadline: Instant,
    mut look: impl FnMut(&Track) -> Result<Seen, String>,
) -> Result<Walked, String> {
    let mut walked = Walked { finished: false, had_lyrics: 0 };
    let left = catch_up(kept_next()?, now, deadline, &mut look)?;
    // A lookup that ended past the deadline leaves no time to start another.
    if !left.is_empty() || Instant::now() >= deadline {
        return Ok(walked);
    }
    let mut cursor = state::get(CURSOR)?.map(|bytes| progress::decode(&bytes)).transpose()?;
    let mut checks = 0;
    loop {
        let page = library::tracks(cursor, PAGE as u32)?;
        let Some(last) = page.last() else { break };
        cursor = Some(last.id);
        for t in &page {
            if t.has_lyrics {
                walked.had_lyrics += 1;
                continue;
            }
            checks += 1;
            if progress::is_due(missed_at(t.id)?, now) {
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

/// Looks up with `look` the tracks in `next`, in order, until `deadline`, dropping each from the
/// list once looked up, or once it has left the library, and keeping the list under [`NEXT`]
/// after each. Keeping it that often costs a host call next to each paced request, and means the
/// tracks looked up before an error or the deadline are not looked up again. Answers what is
/// left.
fn catch_up(
    mut next: Vec<u64>,
    now: u64,
    deadline: Instant,
    look: &mut impl FnMut(&Track) -> Result<Seen, String>,
) -> Result<Vec<u64>, String> {
    while !next.is_empty() {
        let ids = next[..next.len().min(PAGE)].to_vec();
        for t in library::get_tracks(&ids)? {
            if Instant::now() >= deadline {
                return Ok(next);
            }
            note(t.id, look(&t)?, now)?;
            next.retain(|&kept| kept != t.id);
            keep_next(&next)?;
        }
        next.retain(|kept| !ids.contains(kept));
        keep_next(&next)?;
    }
    Ok(next)
}

/// Looks up with `look` the tracks an earlier change left (as [`resume`] does first), except
/// those this change names again, then each of the tracks `changed` names, and forgets any miss
/// on those `removed` names. At `deadline` it stops starting lookups, so a slow service cannot
/// hold the run past its time limit, and keeps the tracks without lyrics it did not reach under
/// [`NEXT`], up to [`MAX_NEXT`], for the next run of the pass to look up first. It forgets any
/// miss on them too, so the pass reaches even those past the limit in its turn. Answers how
/// many it left. An error from `look` ends it, so the event is delivered again.
pub fn changed(
    changed: &[u64],
    removed: &[u64],
    now: u64,
    deadline: Instant,
    mut look: impl FnMut(&Track) -> Result<Seen, String>,
) -> Result<usize, String> {
    let kept = kept_next()?;
    let again = kept.iter().copied().filter(|id| !changed.contains(id)).collect();
    let mut next = catch_up(again, now, deadline, &mut look)?;
    let mut left = Vec::new();
    for ids in changed.chunks(PAGE) {
        for t in library::get_tracks(ids)? {
            if Instant::now() < deadline {
                note(t.id, look(&t)?, now)?;
            } else if !t.has_lyrics {
                state::delete(&progress::miss_key(t.id))?;
                left.push(t.id);
            }
        }
    }
    for &id in removed {
        state::delete(&progress::miss_key(id))?;
    }
    for &id in &left {
        if !next.contains(&id) {
            next.push(id);
        }
    }
    next.truncate(MAX_NEXT);
    if next != kept {
        keep_next(&next)?;
    }
    Ok(left.len())
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

/// The tracks kept under [`NEXT`].
fn kept_next() -> Result<Vec<u64>, String> {
    Ok(state::get(NEXT)?.map(|bytes| progress::decode_ids(&bytes)).transpose()?.unwrap_or_default())
}

/// Keeps `next` under [`NEXT`], or deletes the key once it is empty.
fn keep_next(next: &[u64]) -> Result<(), String> {
    match next {
        [] => state::delete(NEXT)?,
        _ => state::set(NEXT, &progress::encode_ids(next))?,
    }
    Ok(())
}
