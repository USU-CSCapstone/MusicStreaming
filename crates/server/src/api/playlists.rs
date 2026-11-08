//! Playlists: `listPlaylists`, `createPlaylist`, `getPlaylist`, and `deletePlaylist`
//! (`api/openapi.yaml`); the items and duplicates endpoints are in `items`, `editPlaylist` in
//! `edit`.
//!
//! A playlist is its user's own, in one library (`design/database.md` §6). Everything here is
//! scoped to the caller's account as well as the library, so another account, an admin
//! included, finds a playlist as absent as one that never was (`requirements/users.md` §10).
//! A playlist's items are kept in order under sparse keys [`KEY_GAP`] apart. Its artwork is
//! an ID kept on the row (`artwork`) and served by `getImage`.

mod announce;
mod artwork;
mod edit;
mod idempotency;
mod items;
mod keys;
mod representation;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use jewelcase_core::playlist::Op;
use jewelcase_core::{sort_key, words};
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;

pub use announce::Change;
pub use artwork::{all_mosaics, held as holds_artwork, mosaics_of};
pub use edit::edit;
use idempotency::{Answer, Key};
pub use items::{all_items, duplicates, list_items};
pub use representation::Playlist;
use representation::{SELECT, playlist};

use super::extract::{self, Path, Query};
use super::page::{Order, Page, Request, Sort, Source, Total, Unknown};
use super::playables::{self, Playable};
use super::realtime::{Hub, ServerMessage};
use super::scope::Scope;
use super::{Code, Id, Images, Problem};
use crate::db::feed::{self, Kind, Op as FeedOp};
use crate::db::{Database, catalog, new_id, now_ms};

/// How far apart the keys of a playlist's items start, so an item put between two takes the
/// midpoint, 20 times over, before the playlist is renumbered.
pub const KEY_GAP: i64 = 1 << 20;

/// The `sort` values `listPlaylists` supports.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum PlaylistSort {
    Name,
    Created,
    #[default]
    Modified,
    // The spec's `playCount` needs listening data, which is not kept per playlist yet, so it
    // answers `422`.
}

impl PlaylistSort {
    /// The keyset this sort pages by. A pinned playlist's `pin_rank` is 1 and the rest's NULL,
    /// the unknown part of the list, so the pinned come first in either order. Each sort's
    /// columns are one of `playlists`' indexes after `user_id` and `library_id`.
    fn sort(self) -> &'static Sort {
        match self {
            PlaylistSort::Name => &Sort {
                label: "playlists.name",
                columns: &["p.pin_rank", "p.sort_key", "p.id"],
                unknown: Unknown::Null,
            },
            PlaylistSort::Created => &Sort {
                label: "playlists.created",
                columns: &["p.pin_rank", "p.created_at", "p.id"],
                unknown: Unknown::Null,
            },
            PlaylistSort::Modified => &Sort {
                label: "playlists.modified",
                columns: &["p.pin_rank", "p.modified_at", "p.id"],
                unknown: Unknown::Null,
            },
        }
    }
}

/// The parameters `listPlaylists` supports. Any other answers `422` rather than being ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    sort: PlaylistSort,
    #[serde(default)]
    order: Order,
    q: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// The playlists `user` has in `library`, those whose title holds all of `q`'s words, in order
/// if `q` has any.
fn source(user: i64, library: i64, q: Option<&str>) -> Source {
    let mut filter = "p.user_id = ? AND p.library_id = ?".to_owned();
    let mut params = vec![Value::from(user), Value::from(library)];
    let folded = words(q.unwrap_or("")).join(" ");
    if !folded.is_empty() {
        filter += " AND instr(p.folded, ?) > 0";
        params.push(Value::from(folded));
    }
    // Counted for every page: a user has hundreds of playlists at most, all in one index range.
    Source {
        select: SELECT,
        from: "playlists p".to_owned(),
        joins: "",
        filter,
        params,
        total: Total::Counted,
    }
}

/// The caller's playlists in `library`, pinned first, a page at a time.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<Playlist>>, Problem> {
    let request = Request::new(query.sort.sort(), query.order, query.cursor, query.limit)?;
    let q = query.q;
    let page = db
        .read(move |conn| request.read(conn, &source(user, library, q.as_deref()), playlist))
        .await?;
    Ok(Json(page))
}

/// The spec's `PlaylistCreate`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Create {
    title: String,
    description: Option<String>,
    items: Option<Vec<Playable>>,
    from_mix: Option<String>,
}

/// Creates a playlist for the caller in `library` at version 1, holding the tracks `items`
/// expand to in order, and returns it as `201`. A blank title, an item that is not the caller's
/// to add, and `fromMix` are `422`. With an `Idempotency-Key` the caller already used for a
/// request that applied, returns that answer and creates nothing. A playlist that was created is
/// announced to the caller's devices once it is committed.
pub async fn create(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    Key(key): Key,
    extract::Json(body): extract::Json<Create>,
) -> Result<Response, Problem> {
    let title = body.title.trim().to_owned();
    if title.is_empty() {
        return Err(Problem::invalid("title must have something in it."));
    }
    if body.from_mix.is_some() {
        return Err(Problem::invalid("Saving a mix as a playlist is not supported yet."));
    }
    let items = body.items.unwrap_or_default();
    // The expansion is the slow part, so it is read before the writer is taken. A track purged
    // between the two is left out by the write. A retry that was answered before is found here,
    // and again in the write, where a concurrent first attempt would be.
    let asked = key.clone();
    let tracks = db
        .try_read(move |conn| match Answer::find(conn, user, asked.as_deref())? {
            Some(answer) => Ok::<_, Problem>(Err(answer)),
            None => Ok(Ok(playables::expand(conn, user, library, &items)?)),
        })
        .await?;
    let tracks = match tracks {
        Ok(tracks) => tracks,
        Err(answer) => return Ok(answer.into_response()),
    };
    let description = body.description;
    let answer = settle(async move {
        let (answer, change) = db
            .try_write(move |tx| {
                if let Some(answer) = Answer::find(tx, user, key.as_deref())? {
                    return Ok::<_, Problem>((answer, None));
                }
                let (id, now) = (new_id()?, now_ms());
                tx.execute(
                    "INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, \
                 description, created_at, modified_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                    params![
                        id,
                        library,
                        user,
                        title,
                        words(&title).join(" "),
                        sort_key(&title, None).as_bytes(),
                        description,
                        now
                    ],
                )?;
                // In the writer, unlike an edit's plan: the playlist is new, so the work is one
                // add, and no track can be purged between the plan and the write.
                let add = vec![Op::Add { at: None, items: tracks }];
                let plan = edit::plan(tx, library, id, add, now)?;
                edit::write(tx, user, library, id, 1, &plan)?;
                let (count, duration) = plan.totals.unwrap_or_default();
                tx.execute(
                    "UPDATE playlists SET track_count = ?2, duration_us = ?3 WHERE id = ?1",
                    params![id, count, duration],
                )?;
                catalog::playlist_artwork(tx, id)?;
                feed::record_account(tx, user, library, Kind::Playlist, id, 0, FeedOp::Upsert)?;
                // The row was just inserted in this transaction.
                let created =
                    representation::get(tx, user, library, id)?.ok_or_else(Problem::not_found)?;
                let answer = Answer::new(StatusCode::CREATED, &created)?;
                if let Some(key) = &key {
                    answer.store(tx, user, key, now)?;
                }
                Ok((answer, Some(Change::of(created, Vec::new()))))
            })
            .await?;
        announce(&hub, user, change);
        Ok(answer)
    })
    .await?;
    Ok(answer.into_response())
}

/// Runs `work`, a write with what follows its commit (the announcement, the pruning), in a task
/// of its own and waits for it. A client that gives up drops the handler, which would drop the
/// work after the write was queued: the change would commit and never be announced, and a retry
/// is answered from its stored answer, which announces nothing. The task runs on.
async fn settle<T: Send + 'static>(
    work: impl Future<Output = Result<T, Problem>> + Send + 'static,
) -> Result<T, Problem> {
    tokio::spawn(work).await.map_err(|_| Problem::new(Code::Internal))?
}

/// Tells `user`'s devices of `change`, if there is one, which is only for a request that
/// applied: a retry answered from its stored answer has nothing new to say. Call it once the
/// change is committed.
fn announce(hub: &Hub, user: i64, change: Option<Change>) {
    if let Some(change) = change {
        hub.broadcast(user, &ServerMessage::Playlist(change));
    }
}

/// `getPlaylist`: the caller's playlist, or not found when it is not theirs in this library.
pub async fn get(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
    Path((_, Id(id))): Path<(Id, Id)>,
) -> Result<Json<Playlist>, Problem> {
    db.read(move |conn| representation::get(conn, user, library, id))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

/// `deletePlaylist`: deletes the caller's playlist and its items, or not found when it is not
/// theirs in this library. A deletion is announced to the caller's devices once it is committed.
pub async fn delete(
    State(db): State<Arc<Database>>,
    State(images): State<Arc<Images>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    Path((_, Id(id))): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    settle(async move {
        let version = db
            .write(move |tx| {
                let version = tx
                    .prepare_cached(
                        "DELETE FROM playlists WHERE id = ?1 AND user_id = ?2 \
                         AND library_id = ?3 RETURNING version",
                    )?
                    .query_row(params![id, user, library], |row| row.get::<_, i64>(0))
                    .optional()?;
                if version.is_some() {
                    feed::record_playlist_deleted(tx, user, library, id)?;
                }
                Ok(version)
            })
            .await?
            .ok_or_else(Problem::not_found)?;
        // Its mosaics go too, off the request, unless another of the user's playlists shows them.
        tokio::spawn(async move { images.prune_mosaics(&db, library, user).await });
        announce(&hub, user, Some(Change::deleted(library, id, version)));
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
