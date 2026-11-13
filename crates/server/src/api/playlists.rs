//! Playlists: `listPlaylists`, `createPlaylist`, `getPlaylist`, and `deletePlaylist`
//! (`api/openapi.yaml`); the items and duplicates endpoints are in `items`, `editPlaylist` in
//! `edit`.
//!
//! A playlist is its user's own, in one library (`design/database.md` §6). Everything here is
//! scoped to the caller's account as well as the library, so another account, an admin
//! included, finds a playlist as absent as one that never was (`requirements/users.md` §10).
//! A playlist's items are kept in order under sparse keys [`KEY_GAP`] apart
//! (`jewelcase_catalog::mutate`). Its artwork is an ID kept on the row (`artwork`) and served by
//! `getImage`.

mod announce;
mod artwork;
mod edit;
mod items;
mod liked;
mod record;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use jewelcase_catalog::mutate::{Deleted, Writer};
use rusqlite::params;
use serde::Deserialize;

pub use announce::Change;
pub use artwork::{all_mosaics, held as holds_artwork, mosaics_of};
pub use edit::edit;
pub use items::{duplicates, list_items};
pub use jewelcase_catalog::playlists::Playlist;
#[cfg(test)]
use jewelcase_catalog::playlists::source;
use jewelcase_catalog::playlists::{self, PlaylistSort, get as get_playlist};
pub use liked::{announce as announce_liked, like, removal, unlike};
pub use record::record;

use super::extract::{self, Path, Query};
use super::page::{Order, Page, Request};
use super::playables::{self, Playable};
use super::realtime::{Hub, ServerMessage};
use super::scope::Scope;
use super::{Code, Id, Images, Problem};
use crate::db::feed;
use crate::db::{Database, new_id, now_ms};

#[cfg(test)]
use jewelcase_catalog::mutate::KEY_GAP;

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

/// The caller's playlists in `library`, pinned first, a page at a time.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<Playlist>>, Problem> {
    let request = Request::new(query.sort.sort(), query.order, query.cursor, query.limit)?;
    let q = query.q;
    let page =
        db.read(move |conn| playlists::list(conn, user, library, request, q.as_deref())).await?;
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
    /// The ID the client chose, which a device that creates offline uses until the create syncs.
    id: Option<Id>,
}

/// Creates a playlist for the caller in `library` at version 1, holding the tracks `items`
/// expand to in order, and returns it as `201`. A blank title, an item that is not the caller's
/// to add, `fromMix`, and an `id` that is not a positive 63-bit integer are `422`; an `id`
/// already used by any playlist is `409 id_taken`, whoever's it is. A playlist that was created
/// is announced to the caller's devices once it is committed.
pub async fn create(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    extract::Json(body): extract::Json<Create>,
) -> Result<Response, Problem> {
    let title = body.title.trim().to_owned();
    if title.is_empty() {
        return Err(Problem::invalid("title must have something in it."));
    }
    if body.from_mix.is_some() {
        return Err(Problem::invalid("Saving a mix as a playlist is not supported yet."));
    }
    let chosen = match body.id {
        Some(Id(0)) => return Err(Problem::invalid("id must be a positive 63-bit integer.")),
        chosen => chosen.map(|Id(id)| id),
    };
    let items = body.items.unwrap_or_default();
    // The expansion is the slow part, so it is read before the writer is taken. A track purged
    // between the two is left out by the write.
    let tracks = db.try_read(move |conn| playables::expand(conn, user, library, &items)).await?;
    let description = body.description;
    let created = settle(async move {
        let (body, change) = db
            .try_write(move |tx| {
                let id = match chosen {
                    Some(id) => id,
                    None => new_id()?,
                };
                let mut effects = |effect| record(tx, user, library, effect);
                Writer::new(tx, user, library, now_ms(), &mut effects).create_playlist(
                    id,
                    &title,
                    description.as_deref(),
                    tracks,
                )?;
                // The row was just inserted in this transaction.
                let created =
                    get_playlist(tx, user, library, id)?.ok_or_else(Problem::not_found)?;
                let body = serde_json::to_value(&created)?;
                Ok::<_, Problem>((body, Change::of(created, Vec::new())))
            })
            .await?;
        announce(&hub, user, Some(change));
        Ok(body)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(created)).into_response())
}

/// Runs `work`, a write with what follows its commit (the announcement, the pruning), in a task
/// of its own and waits for it. A client that gives up drops the handler, which would drop the
/// work after the write was queued: the change would commit and never be announced, and a retry
/// is answered from its stored answer, which announces nothing. The task runs on.
pub async fn settle<T: Send + 'static>(
    work: impl Future<Output = Result<T, Problem>> + Send + 'static,
) -> Result<T, Problem> {
    tokio::spawn(work).await.map_err(|_| Problem::new(Code::Internal))?
}

/// Tells `user`'s devices of `change`, if there is one, which is only for a request that
/// applied: a retry answered from its stored answer has nothing new to say. Call it once the
/// change is committed.
pub fn announce(hub: &Hub, user: i64, change: Option<Change>) {
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
    db.read(move |conn| get_playlist(conn, user, library, id))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

/// `deletePlaylist`: deletes the caller's playlist and its items, or not found when it is not
/// theirs in this library. Deleting their Liked Songs, which unlikes everything in it, is
/// remembered, so it is not made again until they next like a track. A deletion is announced
/// to the caller's devices once it is committed.
pub async fn delete(
    State(db): State<Arc<Database>>,
    State(images): State<Arc<Images>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    Path((_, Id(id))): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    settle(async move {
        let version = db
            .try_write(move |tx| {
                // Deleting their Liked Songs unlikes everything in it, which its items name
                // until the delete takes them.
                tx.prepare_cached(
                    "INSERT INTO mark_events (user_id, library_id, track_id, kind, created_at) \
                     SELECT ?2, ?3, i.track_id, 'unliked', ?4 FROM playlists p \
                     CROSS JOIN playlist_items i ON i.playlist_id = p.id \
                     WHERE p.id = ?1 AND p.user_id = ?2 AND p.library_id = ?3 AND p.kind = 'liked' \
                     ORDER BY i.key",
                )?
                .execute(params![id, user, library, now_ms()])?;
                let mut effects = |effect| record(tx, user, library, effect);
                let deleted =
                    Writer::new(tx, user, library, now_ms(), &mut effects).delete_playlist(id)?;
                if deleted.is_some() {
                    feed::record_playlist_deleted(tx, user, library, id)?;
                }
                // A Liked Songs deleted, which unliked everything in it, is not made again until
                // the next like (`requirements/playlists.md` §11).
                if let Some(Deleted { liked: true, .. }) = deleted {
                    tx.prepare_cached(
                        "INSERT OR IGNORE INTO liked_songs_deleted (user_id, library_id) \
                         VALUES (?1, ?2)",
                    )?
                    .execute([user, library])?;
                }
                Ok::<_, Problem>(deleted.map(|deleted| deleted.version))
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
