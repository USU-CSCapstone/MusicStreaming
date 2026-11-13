//! `likeTrack` and `unlikeTrack` (`api/openapi.yaml`): a like puts a track at the top of the
//! caller's Liked Songs, made again if they deleted it, and an unlike takes it out
//! (`requirements/playlists.md` §11). Neither is checked against a version: each says whether a
//! track is liked, not where it sits, so it applies whatever else changed the playlist. Nor
//! does either read the playlist: each finds the track's rows through `playlist_items_by_track`,
//! writes only those, and moves the totals by them, so the writer is held as briefly in a Liked
//! Songs of thousands as in an empty one. Each is announced to the caller's devices as an edit
//! is. Liking a hidden track unhides it, and hiding one unlikes it, so the two never contradict
//! each other (`requirements/conventions.md` §8). The rows each changes are written by the code
//! a device shares (`jewelcase_catalog::mutate`); what is here is the server's own.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use jewelcase_catalog::mutate::{Liked, Removed, Writer};
use jewelcase_catalog::playlists;
use rusqlite::Connection;
use serde::Deserialize;

use super::announce::{AppliedOp, Change};
use super::record;
use crate::api::extract::{Path, Query};
use crate::api::hidden;
use crate::api::realtime::{Hub, ServerMessage};
use crate::api::scope::Scope;
use crate::api::{Id, Problem};
use crate::db::{Database, liked_songs, new_id, now_ms};

/// What `likeTrack` takes besides its path.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LikeQuery {
    /// The ID the client chose for the Liked Songs, used only if the like has to make it again.
    liked_id: Option<Id>,
}

/// `likeTrack`: puts the track at the top of the caller's Liked Songs, unless it is in it
/// already, making the playlist again, pinned, under `likedId` if one is given, if they deleted
/// it, and unhides it. A track the library does not have is not found; a `likedId` of 0 is
/// `422`, and one any playlist has already is `409 id_taken`, checked only when it is used.
pub async fn like(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
    Query(LikeQuery { liked_id }): Query<LikeQuery>,
) -> Result<StatusCode, Problem> {
    let chosen = match liked_id {
        Some(Id(0)) => return Err(Problem::invalid("likedId must be a positive 63-bit integer.")),
        chosen => chosen.map(|Id(id)| id),
    };
    super::settle(async move {
        let (change, unhidden) = db
            .try_write(move |tx| {
                let now = now_ms();
                // Made again unless they deleted it, which a like undoes.
                let fresh = match liked_songs::find(tx, user, library)? {
                    Some(_) => None,
                    None => {
                        tx.prepare_cached(
                            "DELETE FROM liked_songs_deleted WHERE user_id = ?1 AND library_id = ?2",
                        )?
                        .execute([user, library])?;
                        Some(match chosen {
                            Some(id) => id,
                            None => new_id()?,
                        })
                    }
                };
                let mut effects = |effect| record(tx, user, library, effect);
                let mut w = Writer::new(tx, user, library, now, &mut effects);
                let Liked { unhidden, added_to } = w.like(track, fresh)?;
                let change = match added_to {
                    Some(id) => {
                        let liked =
                            playlists::get(tx, user, library, id)?.ok_or_else(Problem::not_found)?;
                        let added = AppliedOp::Add {
                            at: Some(0),
                            track_ids: vec![Id(track)],
                            added_at: now,
                        };
                        Some(Change::of(liked, vec![added]))
                    }
                    None => None,
                };
                Ok::<_, Problem>((change, unhidden))
            })
            .await?;
        if change.is_some() {
            announce(&hub, user, library, [track], true);
        }
        super::announce(&hub, user, change);
        if unhidden {
            hidden::announce(&hub, user, library, [track], false);
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `unlikeTrack`: takes the track out of the caller's Liked Songs. One not in it, or with no
/// Liked Songs to be in, is already not liked, which is the answer.
pub async fn unlike(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    super::settle(async move {
        let change = db
            .try_write(move |tx| {
                let mut effects = |effect| record(tx, user, library, effect);
                let mut w = Writer::new(tx, user, library, now_ms(), &mut effects);
                let removed = w.take_out(track)?;
                removed.map(|removed| removal(tx, user, library, removed)).transpose()
            })
            .await?;
        if change.is_some() {
            announce(&hub, user, library, [track], false);
        }
        super::announce(&hub, user, change);
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Tells `user`'s devices that `tracks` of `library` were liked, or unliked, so each shows it on
/// every copy of them it holds, which a change to Liked Songs alone, naming a removed item by its
/// position, would not let it do. Call it once the change is committed.
pub fn announce(
    hub: &Hub,
    user: i64,
    library: i64,
    tracks: impl IntoIterator<Item = i64>,
    liked: bool,
) {
    for track in tracks {
        let message =
            ServerMessage::TrackLiked { library_id: Id(library), track_id: Id(track), liked };
        hub.broadcast(user, &message);
    }
}

/// The change to announce once `removed` was taken out of a Liked Songs.
pub fn removal(
    tx: &Connection,
    user: i64,
    library: i64,
    Removed { playlist, positions }: Removed,
) -> Result<Change, Problem> {
    let liked = playlists::get(tx, user, library, playlist)?.ok_or_else(Problem::not_found)?;
    Ok(Change::of(liked, vec![AppliedOp::Remove { positions }]))
}
