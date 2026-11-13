//! `editPlaylist` (`api/openapi.yaml`): details and ops against the version they were made on.
//!
//! An edit applies whole or not at all, in one write. Off the writer, the ops are applied in
//! memory by the shared core (`jewelcase_core::playlist`) to the playlist's items at the version
//! the edit was made on, and the rows they change are worked out, under keys from
//! [`keys`](super::keys): a [`Plan`]. The write checks the version again and writes only those
//! rows. So the writer is held for writes that scale with the edit, not with the playlist or
//! the number of ops.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use jewelcase_core::playlist::{Op, apply_ops};
use jewelcase_core::{sort_key, words};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Deserializer};

use super::KEY_GAP;
use super::announce::{AppliedOp, Change};
use super::keys::assign;
use super::liked;
use super::representation::{self, header};
use crate::api::extract::{self, Path};
use crate::api::playables::{self, Playable};
use crate::api::realtime::Hub;
use crate::api::scope::Scope;
use crate::api::{Id, Problem};
use crate::api::{Images, hidden};
use crate::db::feed::{self, Kind, Op as FeedOp};
use crate::db::marks::{self, Mark};
use crate::db::{Database, catalog, now_ms};

/// The spec's `PlaylistEdit`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    base_version: i64,
    title: Option<String>,
    /// `Some(None)` is a `null`, which clears the description.
    #[serde(default, deserialize_with = "nullable")]
    description: Option<Option<String>>,
    pinned: Option<bool>,
    ops: Option<Vec<EditOp>>,
}

/// Tells a field sent as `null` from one left out.
fn nullable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

/// The spec's `PlaylistOp`.
#[derive(Clone, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum EditOp {
    Add { items: Vec<Playable>, at: Option<usize> },
    Remove { positions: Vec<usize> },
    Move { positions: Vec<usize>, at: usize },
}

impl Edit {
    /// The edit's title trimmed, or `422` if it changes nothing, has an empty list of ops or an
    /// op with nothing in it, or a blank title.
    fn check(&self) -> Result<Option<String>, Problem> {
        if self.title.is_none()
            && self.description.is_none()
            && self.pinned.is_none()
            && self.ops.is_none()
        {
            return Err(Problem::invalid("An edit must change something."));
        }
        let ops = self.ops.as_deref().unwrap_or(&[]);
        let empty = self.ops.is_some() && ops.is_empty();
        let hollow = ops.iter().any(|op| match op {
            EditOp::Add { items, .. } => items.is_empty(),
            EditOp::Remove { positions } | EditOp::Move { positions, .. } => positions.is_empty(),
        });
        if empty || hollow {
            return Err(Problem::invalid("ops, and what each op names, must not be empty."));
        }
        match self.title.as_deref().map(str::trim) {
            Some("") => Err(Problem::invalid("title must have something in it.")),
            title => Ok(title.map(str::to_owned)),
        }
    }
}

/// A playlist's item as an edit carries it: its stored key, unless the edit added it.
#[derive(Clone, Copy)]
struct Item {
    key: Option<i64>,
    track: i64,
    added_at: i64,
    duration: i64,
}

/// `editPlaylist`: applies the details and ops to the caller's playlist if it is at
/// `baseVersion`, and returns it at the next version. A Liked Songs keeps its title and
/// description, so an edit of either is `422`, and its adds like tracks ([`liked::only_new`]),
/// which unhides them.
/// Otherwise it is edited as any playlist. Another version is `409 version_conflict`,
/// a position out of range, or an item the caller cannot add, `422`, and nothing is applied
/// either way. With an `Idempotency-Key` the caller already used for an edit that applied,
/// returns that answer and applies nothing. An edit that applied is announced to the caller's
/// devices once it is committed, and one that moves the artwork on deletes the old mosaics.
pub async fn edit(
    State(db): State<Arc<Database>>,
    State(images): State<Arc<Images>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    Path((_, Id(id))): Path<(Id, Id)>,
    extract::Json(body): extract::Json<Edit>,
) -> Result<Response, Problem> {
    let title = body.check()?;
    let names = title.is_some() || body.description.is_some();
    let (base, pinned, description) = (body.base_version, body.pinned, body.description);
    let ops = body.ops.unwrap_or_default();
    super::settle(async move {
        // Made again only when a track the edit adds is purged between the read and the write.
        // The next plan leaves that track out, so each attempt adds fewer, and this ends.
        loop {
            let now = now_ms();
            // Read first, off the writer: a stale version, the items an add expands to, and the
            // plan. The write checks the version again, so an edit made in between is a
            // conflict and not a lost update.
            let ops = ops.clone();
            let (plan, liked) = db
                .try_read(move |conn| {
                    at(conn, user, library, id, base)?;
                    let liked = liked::is_liked(conn, id)?;
                    if liked && names {
                        return Err(Problem::invalid(
                            "Liked Songs keeps its title, and has no description.",
                        ));
                    }
                    let ops = ops
                        .into_iter()
                        .map(|op| expand(conn, user, library, op))
                        .collect::<Result<Vec<_>, _>>()?;
                    let ops = if liked { liked::only_new(conn, library, id, ops)? } else { ops };
                    Ok::<_, Problem>((plan(conn, library, id, ops, now)?, liked))
                })
                .await?;
            let (title, description) = (title.clone(), description.clone());
            let moves_items = plan.totals.is_some();
            let applied = db
                .try_write(move |tx| {
                    // Retention keeps the tracks the playlist holds; only an added one can be gone.
                    if !fresh(tx, library, &plan)? {
                        return Ok::<_, Problem>(None);
                    }
                    write(tx, user, library, id, base, &plan)?;
                    // A track Liked Songs gains is liked, and so not hidden; one it loses is
                    // unliked. What the edit changes, so one added and taken out again in the same
                    // edit is neither, since a plugin's word of it outlasts the edit. Unhidden
                    // first, so a plugin applying the marks in order ends on the like, as the like
                    // endpoint's do.
                    let mut unhidden = Vec::new();
                    let mut likes = (Vec::new(), Vec::new());
                    if liked {
                        for &track in &plan.gained {
                            if hidden::unhide_in(tx, user, library, track)? {
                                unhidden.push(track);
                            }
                        }
                        let (gained, lost) =
                            (plan.gained.iter().copied(), plan.lost.iter().copied());
                        marks::record(tx, user, library, gained, Mark::Liked)?;
                        marks::record(tx, user, library, lost, Mark::Unliked)?;
                        likes = (plan.gained.clone(), plan.lost.clone());
                    }
                    // An edit of details alone leaves the items, and so the artwork.
                    if plan.totals.is_some() {
                        catalog::playlist_artwork(tx, id)?;
                    }
                    let (count, duration) = plan.totals.unzip();
                    let new_title = title.as_deref();
                    tx.prepare_cached(
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
                        user,
                        library,
                        new_title,
                        new_title.map(|t| words(t).join(" ")),
                        new_title.map(|t| sort_key(t, None).as_bytes().to_vec()),
                        description.is_some(),
                        description.flatten(),
                        pinned,
                        now,
                        count,
                        duration,
                    ])?;
                    feed::record_account(tx, user, library, Kind::Playlist, id, 0, FeedOp::Upsert)?;
                    let edited = representation::get(tx, user, library, id)?
                        .ok_or_else(Problem::not_found)?;

                    let body = serde_json::to_value(&edited)?;
                    Ok(Some((body, Change::of(edited, plan.applied), unhidden, likes)))
                })
                .await?;
            if let Some((body, change, unhidden, (liked, unliked))) = applied {
                hidden::announce(&hub, user, library, unhidden, false);
                liked::announce(&hub, user, library, liked, true);
                liked::announce(&hub, user, library, unliked, false);
                if moves_items {
                    // The edit may have moved the artwork on; its old mosaics go, off the request.
                    let db = db.clone();
                    tokio::spawn(async move { images.prune_mosaics(&db, library, user).await });
                }
                super::announce(&hub, user, Some(change));
                return Ok((StatusCode::OK, Json(body)).into_response());
            }
        }
    })
    .await
}

/// `404` unless playlist `id` is `user`'s in `library`, and `409 version_conflict` unless it is
/// at `base`.
fn at(conn: &Connection, user: i64, library: i64, id: i64, base: i64) -> Result<(), Problem> {
    let (version, _) = header(conn, user, library, id)?.ok_or_else(Problem::not_found)?;
    if version != base {
        return Err(Problem::version_conflict(version));
    }
    Ok(())
}

/// `op` with an add's playables expanded to track IDs.
fn expand(conn: &Connection, user: i64, library: i64, op: EditOp) -> Result<Op<i64>, Problem> {
    Ok(match op {
        EditOp::Add { items, at } => {
            Op::Add { at, items: playables::expand(conn, user, library, &items)? }
        }
        EditOp::Remove { positions } => Op::Remove { positions },
        EditOp::Move { positions, at } => Op::Move { positions, at },
    })
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

/// What an edit's ops write, worked out from the playlist's items at one version by [`plan`],
/// for [`write`] to write.
pub(super) struct Plan {
    /// The item count and duration once written, or `None` for an edit of details alone, which
    /// leaves them as they are.
    pub(super) totals: Option<(i64, i64)>,
    /// The distinct tracks the ops add.
    pub(super) added: Vec<i64>,
    /// The tracks the playlist holds once written that it did not before, and those it held and
    /// holds no more, each once and in track order: what the edit changes of which tracks it
    /// holds, so a track added and removed again by the same ops is in neither.
    pub(super) gained: Vec<i64>,
    pub(super) lost: Vec<i64>,
    /// The keys of the rows to delete, or `None` to delete them all, for a renumber.
    delete: Option<Vec<i64>>,
    /// The rows to insert, as `(key, track, added_at)`.
    insert: Vec<(i64, i64, i64)>,
    /// The ops as they apply, to announce: an add's tracks are those still in the library.
    pub(super) applied: Vec<AppliedOp>,
}

/// Applies `ops` to the items of playlist `id` as `conn` finds them, and works out the rows that
/// change, the ops as applied, and the totals after, summed from the tracks' durations as they
/// are now. A track `library`
/// no longer has is left out of an add, as create leaves it out. A position out of range is
/// `422`. It only reads, so it runs off the writer; the caller has checked the playlist is the
/// caller's and at the version the ops were made on.
pub(super) fn plan(
    conn: &Connection,
    library: i64,
    id: i64,
    ops: Vec<Op<i64>>,
    now: i64,
) -> Result<Plan, Problem> {
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
    let after = apply_ops(before.clone(), ops).map_err(|e| Problem::invalid(e.to_string()))?;

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
pub(super) fn fresh(tx: &Connection, library: i64, plan: &Plan) -> rusqlite::Result<bool> {
    Ok(durations(tx, library, plan.added.iter().copied())?.len() == plan.added.len())
}

/// Writes `plan`, made from playlist `id` at version `base`, if the playlist is still `user`'s
/// in `library` and at `base`: otherwise `404` or `409 version_conflict`, and nothing is
/// written. Records each item row it deletes or inserts in the account feed. The details,
/// totals and version, and the playlist's own feed row, are the caller's to write.
pub(super) fn write(
    tx: &Connection,
    user: i64,
    library: i64,
    id: i64,
    base: i64,
    plan: &Plan,
) -> Result<(), Problem> {
    // The version moves with every change to the items, so at `base` they are what the plan
    // was made from.
    at(tx, user, library, id, base)?;
    // Deleted before anything is inserted, so no new key meets an old one on its way out.
    let gone: Vec<i64> = match &plan.delete {
        Some(keys) => {
            let mut delete = tx
                .prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1 AND key = ?2")?;
            for key in keys {
                delete.execute(params![id, key])?;
            }
            keys.clone()
        }
        None => tx
            .prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1 RETURNING key")?
            .query_map([id], |row| row.get(0))?
            .collect::<Result<_, _>>()?,
    };
    // Recorded after the deletes and before the inserts, so a key an insert takes again ends as
    // an upsert and not a tombstone.
    let item = |key, op| feed::record_account(tx, user, library, Kind::PlaylistItem, id, key, op);
    for key in gone {
        item(key, FeedOp::Delete)?;
    }
    let mut insert = tx.prepare_cached(
        "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (key, track, added_at) in &plan.insert {
        insert.execute(params![id, key, library, track, added_at])?;
        item(*key, FeedOp::Upsert)?;
    }
    Ok(())
}
