//! `editPlaylist` (`api/openapi.yaml`): details and ops against the version they were made on.
//!
//! An edit applies whole or not at all, in one write. Off the writer, the ops are applied in
//! memory by the shared core (`jewelcase_core::playlist`) to the playlist's items at the version
//! the edit was made on, and the rows they change are worked out, under keys from
//! [`keys`](super::keys): a [`Plan`]. The write checks the version again and writes only those
//! rows. So the writer is held for writes that scale with the edit, not with the playlist or
//! the number of ops.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use jewelcase_core::playlist::{Op, apply_ops};
use jewelcase_core::{sort_key, words};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Deserializer};

use super::KEY_GAP;
use super::announce::{AppliedOp, Change};
use super::idempotency::{Answer, Key};
use super::keys::assign;
use super::representation::{self, header};
use crate::api::Images;
use crate::api::extract::{self, Path};
use crate::api::playables::{self, Playable};
use crate::api::realtime::Hub;
use crate::api::scope::Scope;
use crate::api::{Id, Problem};
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
/// `baseVersion`, and returns it at the next version. Another version is `409 version_conflict`,
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
    Key(key): Key,
    extract::Json(body): extract::Json<Edit>,
) -> Result<Response, Problem> {
    let title = body.check()?;
    let (base, pinned, description) = (body.base_version, body.pinned, body.description);
    let ops = body.ops.unwrap_or_default();
    // Made again only when a track the edit adds is purged between the read and the write.
    // The next plan leaves that track out, so each attempt adds fewer, and this ends.
    loop {
        let now = now_ms();
        // Read first, off the writer: a retry already answered, a stale version, the items an
        // add expands to, and the plan. The write checks the version again, so an edit made in
        // between is a conflict and not a lost update.
        let (asked, ops) = (key.clone(), ops.clone());
        let prepared = db
            .try_read(move |conn| {
                if let Some(answer) = Answer::find(conn, user, asked.as_deref())? {
                    return Ok::<_, Problem>(Err(answer));
                }
                at(conn, user, library, id, base)?;
                let ops = ops
                    .into_iter()
                    .map(|op| expand(conn, user, library, op))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Ok(plan(conn, library, id, ops, now)?))
            })
            .await?;
        let plan = match prepared {
            Ok(plan) => plan,
            Err(answer) => return Ok(answer.into_response()),
        };
        let (key, title, description) = (key.clone(), title.clone(), description.clone());
        let moves_items = plan.totals.is_some();
        let answer = db
            .try_write(move |tx| {
                if let Some(answer) = Answer::find(tx, user, key.as_deref())? {
                    return Ok::<_, Problem>(Some((answer, None)));
                }
                // Retention keeps the tracks the playlist holds; only an added one can be gone.
                if !fresh(tx, library, &plan)? {
                    return Ok(None);
                }
                write(tx, user, library, id, base, &plan)?;
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
                let edited =
                    representation::get(tx, user, library, id)?.ok_or_else(Problem::not_found)?;
                let answer = Answer::new(StatusCode::OK, &edited)?;
                if let Some(key) = &key {
                    answer.store(tx, user, key, now)?;
                }
                Ok(Some((answer, Some(Change::of(edited, plan.applied)))))
            })
            .await?;
        if let Some((answer, change)) = answer {
            if moves_items && change.is_some() {
                // The edit may have moved the artwork on; its old mosaics go, off the request.
                let db = db.clone();
                tokio::spawn(async move { images.prune_mosaics(&db, library, user).await });
            }
            super::announce(&hub, user, change);
            return Ok(answer.into_response());
        }
    }
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
    added: Vec<i64>,
    /// The keys of the rows to delete, or `None` to delete them all, for a renumber.
    delete: Option<Vec<i64>>,
    /// The rows to insert, as `(key, track, added_at)`.
    insert: Vec<(i64, i64, i64)>,
    /// The ops as they apply, to announce: an add's tracks are those still in the library.
    pub(super) applied: Vec<AppliedOp>,
}

/// Applies `ops` to the items of playlist `id` as `conn` finds them, and works out the rows that
/// change, the ops as applied, and the totals after, summed from the tracks' durations as they
/// are now. A track
/// `library` no longer has is left out of an add, as create leaves it out. A position out of
/// range is `422`. It only reads, so it runs off the writer; the caller has checked the
/// playlist is the caller's and at the version the ops were made on.
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
    let removed = before.iter().filter_map(|item| item.key).filter(|key| !kept.contains(key));
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
/// written. The details, totals and version are the caller's to write.
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
    match &plan.delete {
        Some(keys) => {
            let mut delete = tx
                .prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1 AND key = ?2")?;
            for key in keys {
                delete.execute(params![id, key])?;
            }
        }
        None => {
            tx.prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1")?
                .execute([id])?;
        }
    }
    let mut insert = tx.prepare_cached(
        "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (key, track, added_at) in &plan.insert {
        insert.execute(params![id, key, library, track, added_at])?;
    }
    Ok(())
}
