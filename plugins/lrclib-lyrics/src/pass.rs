//! The pass through the library, a part per run, the tracks a change names, and what the plugin
//! remembers of each track it looked up, kept in its `state`. Written against a [`Store`], which
//! in a run is the plugin's imports, so it is tested natively with one in memory.

use std::time::Instant;

use crate::progress::{self, CHECKS_PER_RUN, CURSOR, MAX_NEXT, Miss, NEXT, Saving, Seen};

/// Tracks read from the library at a time: the most the host gives.
const PAGE: usize = 500;

/// What the pass reads and keeps: the library's tracks, and the plugin's `state`.
pub trait Store {
    /// A track as the library gives it.
    type Track;

    /// The track's ID.
    fn id(track: &Self::Track) -> u64;
    /// Whether the track already has lyrics.
    fn has_lyrics(track: &Self::Track) -> bool;
    /// Up to `limit` tracks after the one with ID `after`, in ID order.
    fn tracks(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Self::Track>, String>;
    /// The tracks with these IDs that are still in the library.
    fn get_tracks(&mut self, ids: &[u64]) -> Result<Vec<Self::Track>, String>;
    /// The value kept under `key` in `state`, if there is one.
    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String>;
    /// Keeps `value` under `key` in `state`, replacing any value there.
    fn set(&mut self, key: &str, value: &[u8]) -> Result<(), String>;
    /// Deletes `key` from `state`.
    fn delete(&mut self, key: &str) -> Result<(), String>;
}

/// How far a run of the pass got.
pub struct Walked {
    /// Whether it reached the end of the library, where the next run starts over.
    pub finished: bool,
    /// The tracks with lyrics it went by.
    pub had_lyrics: u32,
}

/// Goes on with the pass, looking up with `look` first the tracks a change left for it
/// ([`changed`]), then, from where it last stopped, each track without lyrics, except misses
/// not [`Miss::due`] for a run that saves as `saving` says. It stops starting lookups at
/// `deadline`, and stops after [`CHECKS_PER_RUN`] checks. Its place is kept after each lookup,
/// so a run cut short loses nothing it did. An error from `look` ends it before that track, so
/// the next run looks it up first.
pub fn resume<S: Store>(
    store: &mut S,
    now: u64,
    saving: Saving,
    deadline: Instant,
    mut look: impl FnMut(&S::Track) -> Result<Seen, String>,
) -> Result<Walked, String> {
    let mut walked = Walked { finished: false, had_lyrics: 0 };
    let next = kept_next(store)?;
    let left = catch_up(store, next, now, deadline, &mut look)?;
    // A lookup that ended past the deadline leaves no time to start another.
    if !left.is_empty() || Instant::now() >= deadline {
        return Ok(walked);
    }
    let mut cursor = store.get(CURSOR)?.map(|bytes| progress::decode(&bytes)).transpose()?;
    let mut checks = 0;
    loop {
        let page = store.tracks(cursor, PAGE as u32)?;
        let Some(last) = page.last() else { break };
        cursor = Some(S::id(last));
        for t in &page {
            let id = S::id(t);
            if S::has_lyrics(t) {
                walked.had_lyrics += 1;
                continue;
            }
            checks += 1;
            if progress::wanted(miss(store, id)?, now, saving) {
                note(store, id, look(t)?, now)?;
                store.set(CURSOR, &progress::encode(id))?;
                // Read only after a lookup: reading the clock is a host call, which costs a tick.
                if Instant::now() >= deadline {
                    return Ok(walked);
                }
            }
            if checks == CHECKS_PER_RUN {
                store.set(CURSOR, &progress::encode(id))?;
                return Ok(walked);
            }
        }
    }
    store.delete(CURSOR)?;
    walked.finished = true;
    Ok(walked)
}

/// Looks up with `look` the tracks in `next`, in order, until `deadline`, dropping each from the
/// list once looked up, or once it has left the library, and keeping the list under [`NEXT`]
/// after each. Keeping it that often costs a host call next to each paced request, and means the
/// tracks looked up before an error or the deadline are not looked up again. Answers what is
/// left.
fn catch_up<S: Store>(
    store: &mut S,
    mut next: Vec<u64>,
    now: u64,
    deadline: Instant,
    look: &mut impl FnMut(&S::Track) -> Result<Seen, String>,
) -> Result<Vec<u64>, String> {
    while !next.is_empty() {
        let ids = next[..next.len().min(PAGE)].to_vec();
        for t in store.get_tracks(&ids)? {
            if Instant::now() >= deadline {
                return Ok(next);
            }
            let id = S::id(&t);
            note(store, id, look(&t)?, now)?;
            next.retain(|&kept| kept != id);
            keep_next(store, &next)?;
        }
        next.retain(|kept| !ids.contains(kept));
        keep_next(store, &next)?;
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
pub fn changed<S: Store>(
    store: &mut S,
    changed: &[u64],
    removed: &[u64],
    now: u64,
    deadline: Instant,
    mut look: impl FnMut(&S::Track) -> Result<Seen, String>,
) -> Result<usize, String> {
    let kept = kept_next(store)?;
    let again = kept.iter().copied().filter(|id| !changed.contains(id)).collect();
    let mut next = catch_up(store, again, now, deadline, &mut look)?;
    let mut left = Vec::new();
    for ids in changed.chunks(PAGE) {
        for t in store.get_tracks(ids)? {
            let id = S::id(&t);
            if Instant::now() < deadline {
                note(store, id, look(&t)?, now)?;
            } else if !S::has_lyrics(&t) {
                forget(store, id)?;
                left.push(id);
            }
        }
    }
    removed.iter().try_for_each(|&id| forget(store, id))?;
    for &id in &left {
        if !next.contains(&id) {
            next.push(id);
        }
    }
    next.truncate(MAX_NEXT);
    if next != kept {
        keep_next(store, &next)?;
    }
    Ok(left.len())
}

/// Remembers what looking up track `id` at `now` came to.
fn note<S: Store>(store: &mut S, id: u64, seen: Seen, now: u64) -> Result<(), String> {
    match seen {
        Seen::Missed(why) => store.set(&progress::miss_key(id), &Miss::new(now, why).encode()),
        Seen::Settled => forget(store, id),
        Seen::Failed => Ok(()),
    }
}

/// Forgets any miss on track `id`, such as once it has lyrics or has left the library.
fn forget<S: Store>(store: &mut S, id: u64) -> Result<(), String> {
    let key = progress::miss_key(id);
    match store.get(&key)? {
        Some(_) => store.delete(&key),
        None => Ok(()),
    }
}

/// The miss kept on track `id`, if there is one.
fn miss<S: Store>(store: &mut S, id: u64) -> Result<Option<Miss>, String> {
    store.get(&progress::miss_key(id))?.map(|bytes| Miss::decode(&bytes)).transpose()
}

/// The tracks kept under [`NEXT`].
fn kept_next<S: Store>(store: &mut S) -> Result<Vec<u64>, String> {
    Ok(store.get(NEXT)?.map(|bytes| progress::decode_ids(&bytes)).transpose()?.unwrap_or_default())
}

/// Keeps `next` under [`NEXT`], or deletes the key once it is empty.
fn keep_next<S: Store>(store: &mut S, next: &[u64]) -> Result<(), String> {
    match next {
        [] => store.delete(NEXT),
        _ => store.set(NEXT, &progress::encode_ids(next)),
    }
}

#[cfg(test)]
mod tests;
