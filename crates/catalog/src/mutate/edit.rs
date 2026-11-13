//! Editing a playlist's items and details: the ops applied in memory by the shared core
//! (`jewelcase_core::playlist`) to the playlist's items at the version the edit was made on, the
//! rows they change worked out under keys from [`keys`](super::keys): a [`Plan`], then written. The
//! plan only reads, so the server makes it off its writer, which is held only for writes that
//! scale with the edit, not with the playlist or the number of ops.

use std::collections::{BTreeSet, HashMap, HashSet};

use jewelcase_core::playlist::{Op, apply_ops};
use jewelcase_core::{sort_key, words};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::keys::assign;
use super::{Effect, Error, KEY_GAP, Writer};
use crate::sync::Kind;
use crate::time::serialize_rfc3339;
use crate::{Id, liked, playlists};

/// The spec's `PlaylistAppliedOp`: an op as it applied. An add holds the tracks it expanded to,
/// less any purged meanwhile, and the time they were added.
#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum AppliedOp {
    Add {
        #[serde(skip_serializing_if = "Option::is_none")]
        at: Option<usize>,
        track_ids: Vec<Id>,
        #[serde(serialize_with = "serialize_rfc3339")]
        added_at: i64,
    },
    Remove {
        positions: Vec<usize>,
    },
    Move {
        positions: Vec<usize>,
        at: usize,
    },
}

/// A playlist's item as an edit carries it: its stored key, unless the edit added it.
#[derive(Clone, Copy)]
struct Item {
    key: Option<i64>,
    track: i64,
    added_at: i64,
    duration: i64,
}

/// What an edit's ops write, worked out from the playlist's items at one version by [`plan`],
/// for [`write`](fn@write) to write.
#[derive(Debug)]
pub struct Plan {
    /// The item count and duration once written, or `None` for an edit of details alone, which
    /// leaves them as they are.
    pub totals: Option<(i64, i64)>,
    /// The distinct tracks the ops add.
    pub added: Vec<i64>,
    /// The tracks the playlist holds once written that it did not before, and those it held and
    /// holds no more, each once and in track order: what the edit changes of which tracks it
    /// holds, so a track added and removed again by the same ops is in neither.
    pub gained: Vec<i64>,
    pub lost: Vec<i64>,
    /// The keys of the rows to delete, or `None` to delete them all, for a renumber.
    delete: Option<Vec<i64>>,
    /// The rows to insert, as `(key, track, added_at)`.
    insert: Vec<(i64, i64, i64)>,
    /// The ops as they apply: an add's tracks are those still in the library.
    pub applied: Vec<AppliedOp>,
}

/// An edit's details: what it changes of the playlist besides its items.
pub struct Details {
    /// The new title, trimmed.
    pub title: Option<String>,
    /// `Some(None)` clears the description.
    pub description: Option<Option<String>>,
    pub pinned: Option<bool>,
}

/// The edit's title trimmed, or [`Error::Invalid`] if it changes nothing, has an empty list of
/// ops or an op with nothing in it, or a blank title. `details` is whether it sets the
/// description or the pin, and `ops` how many items or positions each of its ops names, if it has
/// ops.
pub fn check_edit(
    title: Option<&str>,
    details: bool,
    ops: Option<&[usize]>,
) -> Result<Option<String>, Error> {
    if title.is_none() && !details && ops.is_none() {
        return Err(Error::Invalid("An edit must change something.".to_owned()));
    }
    let empty = ops.is_some_and(<[usize]>::is_empty);
    let hollow = ops.is_some_and(|ops| ops.contains(&0));
    if empty || hollow {
        return Err(Error::Invalid("ops, and what each op names, must not be empty.".to_owned()));
    }
    match title.map(str::trim) {
        Some("") => Err(Error::Invalid("title must have something in it.".to_owned())),
        title => Ok(title.map(str::to_owned)),
    }
}

/// [`Error::NotFound`] unless playlist `id` is `user`'s in `library`, and
/// [`Error::VersionConflict`] unless it is at `base`, when there is one.
pub fn at(
    conn: &Connection,
    user: i64,
    library: i64,
    id: i64,
    base: Option<i64>,
) -> Result<(), Error> {
    let (version, _) = playlists::header(conn, user, library, id)?.ok_or(Error::NotFound)?;
    match base {
        Some(base) if base != version => Err(Error::VersionConflict(version)),
        _ => Ok(()),
    }
}

/// What an edit of playlist `id` is checked against before anything is read of its items: `at`,
/// and a Liked Songs keeps its title and has no description, so an edit that `names` either is
/// invalid. Returns whether it is a Liked Songs.
pub fn check(
    conn: &Connection,
    user: i64,
    library: i64,
    id: i64,
    base: Option<i64>,
    names: bool,
) -> Result<bool, Error> {
    at(conn, user, library, id, base)?;
    let liked = liked::is_liked(conn, id)?;
    if liked && names {
        return Err(Error::Invalid(
            "Liked Songs keeps its title, and has no description.".to_owned(),
        ));
    }
    Ok(liked)
}

/// [`plan`] for an edit's `ops` of playlist `id`, which are those of a Liked Songs when `liked`:
/// there adding a track likes it, so an add with no position goes to the top, as a like does, and
/// adds only the tracks it does not hold, each once. A track it holds as the edit is made is not
/// added again, even after a remove in the same edit.
pub fn plan_for(
    conn: &Connection,
    library: i64,
    id: i64,
    liked: bool,
    ops: Vec<Op<i64>>,
    now: i64,
) -> Result<Plan, Error> {
    let ops = if liked { only_new(conn, library, id, ops)? } else { ops };
    plan(conn, library, id, ops, now)
}

/// `ops` as they apply to Liked Songs `id` of `library`, as [`plan_for`] says.
fn only_new(
    conn: &Connection,
    library: i64,
    id: i64,
    ops: Vec<Op<i64>>,
) -> rusqlite::Result<Vec<Op<i64>>> {
    let mut added = HashSet::new();
    let mut only_new = Vec::with_capacity(ops.len());
    for op in ops {
        only_new.push(match op {
            Op::Add { at, items } => {
                let mut new = Vec::new();
                for track in items {
                    if added.insert(track) && !super::like::holds(conn, library, id, track)? {
                        new.push(track);
                    }
                }
                Op::Add { at: at.or(Some(0)), items: new }
            }
            op => op,
        });
    }
    Ok(only_new)
}

/// The duration of each of `tracks` that `library` still has.
fn durations(
    conn: &Connection,
    library: i64,
    tracks: impl IntoIterator<Item = i64>,
) -> rusqlite::Result<HashMap<i64, i64>> {
    let mut find =
        conn.prepare_cached("SELECT duration_us FROM tracks WHERE library_id = ?1 AND id = ?2")?;
    let mut found = HashMap::new();
    for track in tracks {
        if let Some(duration) =
            find.query_row(params![library, track], |row| row.get(0)).optional()?
        {
            found.insert(track, duration);
        }
    }
    Ok(found)
}

/// Applies `ops` to the items of playlist `id` as `conn` finds them, and works out the rows that
/// change, the ops as applied, and the totals after, summed from the tracks' durations as they
/// are now. A track `library` no longer has is left out of an add, as create leaves it out. A
/// position out of range is [`Error::Invalid`]. It only reads; the caller has checked the
/// playlist is the caller's and at the version the ops were made on.
pub fn plan(
    conn: &Connection,
    library: i64,
    id: i64,
    ops: Vec<Op<i64>>,
    now: i64,
) -> Result<Plan, Error> {
    if ops.is_empty() {
        return Ok(Plan {
            totals: None,
            added: Vec::new(),
            gained: Vec::new(),
            lost: Vec::new(),
            delete: Some(Vec::new()),
            insert: Vec::new(),
            applied: Vec::new(),
        });
    }
    let added = durations(
        conn,
        library,
        ops.iter()
            .flat_map(|op| match op {
                Op::Add { items, .. } => items.as_slice(),
                _ => &[],
            })
            .copied(),
    )?;
    let ops = ops
        .into_iter()
        .map(|op| match op {
            Op::Add { at, items } => Op::Add {
                at,
                items: items
                    .into_iter()
                    .filter(|track| added.contains_key(track))
                    .map(|track| Item { key: None, track, added_at: now, duration: added[&track] })
                    .collect(),
            },
            Op::Remove { positions } => Op::Remove { positions },
            Op::Move { positions, at } => Op::Move { positions, at },
        })
        .collect::<Vec<Op<Item>>>();
    let applied = ops
        .iter()
        .map(|op| match op {
            Op::Add { at, items } => AppliedOp::Add {
                at: *at,
                track_ids: items.iter().map(|item| Id(item.track)).collect(),
                added_at: now,
            },
            Op::Remove { positions } => AppliedOp::Remove { positions: positions.clone() },
            Op::Move { positions, at } => AppliedOp::Move { positions: positions.clone(), at: *at },
        })
        .collect();
    let before: Vec<Item> = conn
        .prepare_cached(
            "SELECT i.key, i.track_id, i.added_at, t.duration_us FROM playlist_items i \
             JOIN tracks t ON t.library_id = i.library_id AND t.id = i.track_id \
             WHERE i.playlist_id = ?1 ORDER BY i.key",
        )?
        .query_map([id], |row| {
            Ok(Item {
                key: Some(row.get(0)?),
                track: row.get(1)?,
                added_at: row.get(2)?,
                duration: row.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let after = apply_ops(before.clone(), ops).map_err(|e| Error::Invalid(e.to_string()))?;

    let stored: Vec<Option<i64>> = after.iter().map(|item| item.key).collect();
    let kept: HashSet<i64> = stored.iter().flatten().copied().collect();
    let gone = |item: &&Item| item.key.is_some_and(|key| !kept.contains(&key));
    let held = |items: &[Item]| items.iter().map(|item| item.track).collect::<BTreeSet<i64>>();
    let (held_before, held_after) = (held(&before), held(&after));
    let gained = held_after.difference(&held_before).copied().collect();
    let lost = held_before.difference(&held_after).copied().collect();
    let removed = before.iter().filter(gone).filter_map(|item| item.key);
    let duration = after.iter().map(|item| item.duration).sum::<i64>();
    let count = i64::try_from(after.len()).unwrap_or(i64::MAX);
    let (delete, insert) = match assign(&stored) {
        Some(keys) => {
            let written: Vec<(&Item, i64)> =
                after.iter().zip(keys).filter(|(item, key)| item.key != Some(*key)).collect();
            let moved = written.iter().filter_map(|(item, _)| item.key);
            let delete = removed.chain(moved).collect();
            (
                Some(delete),
                written.iter().map(|(item, key)| (*key, item.track, item.added_at)).collect(),
            )
        }
        // No room between two neighbours: the whole playlist takes fresh keys, spaced out again.
        None => (
            None,
            after
                .iter()
                .zip(1..)
                .map(|(item, n)| (n * KEY_GAP, item.track, item.added_at))
                .collect(),
        ),
    };
    Ok(Plan {
        totals: Some((count, duration)),
        added: added.into_keys().collect(),
        gained,
        lost,
        delete,
        insert,
        applied,
    })
}

/// Whether every track `plan` adds is still in `library`, which a plan made off the writer must
/// ask in the writer before it is written: one purged since is `false`, and the plan is to be
/// made again. A plan made in the writer itself needs no such check.
pub fn fresh(conn: &Connection, library: i64, plan: &Plan) -> rusqlite::Result<bool> {
    Ok(durations(conn, library, plan.added.iter().copied())?.len() == plan.added.len())
}

/// Writes `plan`, made from playlist `id` at version `base` if there is one, if the playlist is
/// still the writer's user's in its library and at `base`: otherwise [`Error::NotFound`] or
/// [`Error::VersionConflict`], and nothing is written. Reports each item row it deletes or
/// inserts. The details, totals and version are the caller's to write.
pub fn write(w: &mut Writer, id: i64, base: Option<i64>, plan: &Plan) -> Result<(), Error> {
    // The version moves with every change to the items, so at `base` they are what the plan
    // was made from.
    at(w.conn, w.user, w.library, id, base)?;
    // Deleted before anything is inserted, so no new key meets an old one on its way out.
    let gone: Vec<i64> = match &plan.delete {
        Some(keys) => {
            let mut delete = w
                .conn
                .prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1 AND key = ?2")?;
            for key in keys {
                w.touch(Kind::PlaylistItem, id, *key)?;
                delete.execute(params![id, key])?;
            }
            keys.clone()
        }
        None => {
            if w.is_recording() {
                let keys: Vec<i64> = w
                    .conn
                    .prepare_cached("SELECT key FROM playlist_items WHERE playlist_id = ?1")?
                    .query_map([id], |row| row.get(0))?
                    .collect::<Result<_, _>>()?;
                for key in keys {
                    w.touch(Kind::PlaylistItem, id, key)?;
                }
            }
            w.conn
                .prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1 RETURNING key")?
                .query_map([id], |row| row.get(0))?
                .collect::<Result<_, _>>()?
        }
    };
    // Reported after the deletes and before the inserts, so a key an insert takes again ends as
    // an upsert and not a tombstone.
    for key in gone {
        w.report(Effect::Item { playlist: id, key, kept: false })?;
    }
    for (key, track, added_at) in &plan.insert {
        w.touch(Kind::PlaylistItem, id, *key)?;
        w.conn
            .prepare_cached(
                "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![id, key, w.library, track, added_at])?;
        w.report(Effect::Item { playlist: id, key: *key, kept: true })?;
    }
    Ok(())
}

/// Writes an edit's `plan` of playlist `id` (see [`write`](fn@write)) and then its `details`: a
/// Liked Songs (`liked`) gains tracks that are liked, and so not hidden, and loses tracks that are
/// unliked, which is what the edit changes, so one added and taken out again in the same edit is
/// neither. A change to the items moves the artwork on; the details, the totals the plan found
/// and the next version are written, and the playlist reported.
pub fn write_edit(
    w: &mut Writer,
    id: i64,
    base: Option<i64>,
    plan: &Plan,
    details: &Details,
    liked: bool,
) -> Result<(), Error> {
    write(w, id, base, plan)?;
    if liked {
        // Unhidden first, so a plugin applying the marks in order ends on the like.
        for &track in &plan.gained {
            w.unhide(track)?;
        }
        for &track in &plan.gained {
            w.report(Effect::Liked { track, liked: true })?;
        }
        for &track in &plan.lost {
            w.report(Effect::Liked { track, liked: false })?;
        }
    }
    // An edit of details alone leaves the items, and so the artwork.
    if plan.totals.is_some() {
        w.artwork(id)?;
    }
    w.touch(Kind::Playlist, id, 0)?;
    let (count, duration) = plan.totals.unzip();
    let Details { title, description, pinned } = details;
    let title = title.as_deref();
    w.conn
        .prepare_cached(
            "UPDATE playlists SET title = coalesce(?4, title), \
             folded = coalesce(?5, folded), sort_key = coalesce(?6, sort_key), \
             description = CASE WHEN ?7 THEN ?8 ELSE description END, \
             pinned = coalesce(?9, pinned), version = version + 1, modified_at = ?10, \
             track_count = coalesce(?11, track_count), \
             duration_us = coalesce(?12, duration_us) \
             WHERE id = ?1 AND user_id = ?2 AND library_id = ?3",
        )?
        .execute(params![
            id,
            w.user,
            w.library,
            title,
            title.map(|t| words(t).join(" ")),
            title.map(|t| sort_key(t, None).as_bytes().to_vec()),
            description.is_some(),
            description.clone().flatten(),
            pinned,
            w.now,
            count,
            duration,
        ])?;
    w.report(Effect::Playlist { id })?;
    Ok(())
}
