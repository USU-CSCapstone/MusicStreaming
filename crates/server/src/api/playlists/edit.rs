//! `editPlaylist` (`api/openapi.yaml`): details and ops against the version they were made on.
//!
//! An edit applies whole or not at all, in one write: the version is checked, the ops are
//! applied in memory by the shared core (`jewelcase_core::playlist`), and then only the rows the
//! ops changed are written, under keys from [`keys`](super::keys). So the writer is held for one
//! read of the playlist's items and for writes that scale with the edit, not with the playlist.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use jewelcase_core::playlist::{Op, apply_ops};
use jewelcase_core::{sort_key, words};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Deserializer};

use super::idempotency::{Answer, Key};
use super::items::header;
use super::keys::assign;
use super::{KEY_GAP, representation};
use crate::api::extract::{self, Path};
use crate::api::playables::{self, Playable};
use crate::api::scope::Scope;
use crate::api::{Id, Problem};
use crate::db::{Database, now_ms};

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
#[derive(Deserialize)]
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
/// returns that answer and applies nothing.
pub async fn edit(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
    Path((_, Id(id))): Path<(Id, Id)>,
    Key(key): Key,
    extract::Json(body): extract::Json<Edit>,
) -> Result<Response, Problem> {
    let title = body.check()?;
    let (base, pinned, description) = (body.base_version, body.pinned, body.description);
    let ops = body.ops.unwrap_or_default();
    // Read first, off the writer: a retry already answered, a stale version, and the items an
    // add expands to. The write checks the version again, so an edit made in between is a
    // conflict and not a lost update.
    let asked = key.clone();
    let prepared = db
        .try_read(move |conn| {
            if let Some(answer) = Answer::find(conn, user, asked.as_deref())? {
                return Ok::<_, Problem>(Err(answer));
            }
            let (version, _) = header(conn, user, library, id)?.ok_or_else(Problem::not_found)?;
            if version != base {
                return Err(Problem::version_conflict(version));
            }
            ops.into_iter()
                .map(|op| expand(conn, user, library, op))
                .collect::<Result<Vec<_>, _>>()
                .map(Ok)
        })
        .await?;
    let ops = match prepared {
        Ok(ops) => ops,
        Err(answer) => return Ok(answer.into_response()),
    };
    let answer = db
        .try_write(move |tx| {
            if let Some(answer) = Answer::find(tx, user, key.as_deref())? {
                return Ok::<_, Problem>(answer);
            }
            let now = now_ms();
            let (count, duration) = update(tx, user, library, id, base, ops, now)?;
            let new_title = title.as_deref();
            tx.prepare_cached(
                "UPDATE playlists SET title = coalesce(?4, title), folded = coalesce(?5, folded), \
                 sort_key = coalesce(?6, sort_key), \
                 description = CASE WHEN ?7 THEN ?8 ELSE description END, \
                 pinned = coalesce(?9, pinned), version = version + 1, modified_at = ?10, \
                 track_count = ?11, duration_us = ?12 \
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
            Ok(answer)
        })
        .await?;
    Ok(answer.into_response())
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

/// Checks the playlist is the caller's and at `base`, applies `ops` to its items, writes the
/// rows that changed, and returns the playlist's new item count and duration, summed from the
/// tracks as they are now. A position out of range is `422`. The details and version are the
/// caller's to write.
fn update(
    tx: &Transaction,
    user: i64,
    library: i64,
    id: i64,
    base: i64,
    ops: Vec<Op<i64>>,
    now: i64,
) -> Result<(i64, i64), Problem> {
    let (version, count, duration): (i64, i64, i64) = tx
        .prepare_cached(
            "SELECT version, track_count, duration_us FROM playlists \
             WHERE id = ?1 AND user_id = ?2 AND library_id = ?3",
        )?
        .query_row(params![id, user, library], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Problem::not_found(),
            error => error.into(),
        })?;
    if version != base {
        return Err(Problem::version_conflict(version));
    }
    if ops.is_empty() {
        return Ok((count, duration));
    }

    // A track purged since the expansion is left out of the add, as create leaves it out.
    let added = durations(
        tx,
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
        .collect();
    let before: Vec<Item> = tx
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
    let removed: Vec<&Item> =
        before.iter().filter(|item| item.key.is_some_and(|key| !kept.contains(&key))).collect();
    let (count, duration) = (after.len(), after.iter().map(|item| item.duration).sum::<i64>());
    let count = i64::try_from(count).unwrap_or(i64::MAX);

    // Deleted before anything is inserted, so no new key meets an old one that is on its way out.
    let mut delete =
        tx.prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1 AND key = ?2")?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    match assign(&stored) {
        Some(keys) => {
            let written: Vec<(&Item, i64)> =
                after.iter().zip(keys).filter(|(item, key)| item.key != Some(*key)).collect();
            for key in
                removed.iter().chain(written.iter().map(|(item, _)| item)).filter_map(|i| i.key)
            {
                delete.execute(params![id, key])?;
            }
            for (item, key) in written {
                insert.execute(params![id, key, library, item.track, item.added_at])?;
            }
        }
        None => {
            // No room between two neighbours: the whole playlist takes fresh keys, spaced out
            // again.
            tx.prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1")?
                .execute([id])?;
            for (item, number) in after.iter().zip(1..) {
                insert.execute(params![
                    id,
                    number * KEY_GAP,
                    library,
                    item.track,
                    item.added_at
                ])?;
            }
        }
    }
    Ok((count, duration))
}
