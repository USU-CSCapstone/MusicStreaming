//! `editPlaylist` (`api/openapi.yaml`): details and ops against the version they were made on.
//!
//! An edit applies whole or not at all, in one write. Off the writer, the ops are applied in
//! memory by the shared core (`jewelcase_core::playlist`) to the playlist's items at the version
//! the edit was made on, and the rows they change are worked out, under keys from
//! [`keys`](super::keys): a [`Plan`]. The write checks the version again and writes only those
//! rows. So the writer is held for writes that scale with the edit, not with the playlist or
//! the number of ops.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use jewelcase_catalog::mutate::{self, Details, Effect, Writer, check, check_edit, plan_for};
use jewelcase_core::playlist::Op;
use rusqlite::Connection;
use serde::{Deserialize, Deserializer};

use super::announce::Change;
use super::{liked, record};
use crate::api::extract::{self, Path};
use crate::api::playables::{self, Playable};
use crate::api::realtime::Hub;
use crate::api::scope::Scope;
use crate::api::{Id, Problem};
use crate::api::{Images, hidden};
use crate::db::{Database, now_ms};
use jewelcase_catalog::playlists;

#[cfg(test)]
pub(super) use mutate::Plan;
pub(super) use mutate::fresh;

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
        let sizes: Option<Vec<usize>> = self.ops.as_ref().map(|ops| {
            ops.iter()
                .map(|op| match op {
                    EditOp::Add { items, .. } => items.len(),
                    EditOp::Remove { positions } | EditOp::Move { positions, .. } => {
                        positions.len()
                    }
                })
                .collect()
        });
        let details = self.description.is_some() || self.pinned.is_some();
        Ok(check_edit(self.title.as_deref(), details, sizes.as_deref())?)
    }
}

/// `editPlaylist`: applies the details and ops to the caller's playlist if it is at
/// `baseVersion`, and returns it at the next version. A Liked Songs keeps its title and
/// description, so an edit of either is `422`, and its adds like tracks ([`mutate::plan_for`]),
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
                    let liked = check(conn, user, library, id, Some(base), names)?;
                    let ops = ops
                        .into_iter()
                        .map(|op| expand(conn, user, library, op))
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok::<_, Problem>((plan_for(conn, library, id, liked, ops, now)?, liked))
                })
                .await?;
            let details =
                Details { title: title.clone(), description: description.clone(), pinned };
            let moves_items = plan.totals.is_some();
            let applied = db
                .try_write(move |tx| {
                    // Retention keeps the tracks the playlist holds; only an added one can be gone.
                    if !fresh(tx, library, &plan)? {
                        return Ok::<_, Problem>(None);
                    }
                    let mut unhidden = Vec::new();
                    let mut effects = |effect| {
                        if let Effect::Hidden { track, hidden: false } = effect {
                            unhidden.push(track);
                        }
                        record(tx, user, library, effect)
                    };
                    let mut w = Writer::new(tx, user, library, now, &mut effects);
                    mutate::write_edit(&mut w, id, Some(base), &plan, &details, liked)?;
                    // A track Liked Songs gains is liked, and so not hidden; one it loses is
                    // unliked.
                    let likes = if liked {
                        (plan.gained.clone(), plan.lost.clone())
                    } else {
                        (Vec::new(), Vec::new())
                    };
                    let edited =
                        playlists::get(tx, user, library, id)?.ok_or_else(Problem::not_found)?;
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

/// [`mutate::plan`] with the refusals as the API gives them.
#[cfg(test)]
pub(super) fn plan(
    conn: &Connection,
    library: i64,
    id: i64,
    ops: Vec<Op<i64>>,
    now: i64,
) -> Result<Plan, Problem> {
    Ok(mutate::plan(conn, library, id, ops, now)?)
}

/// Writes `plan`, made from playlist `id` at version `base`, as the API records it.
#[cfg(test)]
pub(super) fn write(
    tx: &Connection,
    user: i64,
    library: i64,
    id: i64,
    base: i64,
    plan: &Plan,
) -> Result<(), Problem> {
    let mut effects = |effect| record(tx, user, library, effect);
    let mut w = Writer::new(tx, user, library, now_ms(), &mut effects);
    Ok(mutate::write(&mut w, id, Some(base), plan)?)
}
