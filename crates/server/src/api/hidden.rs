//! Hidden tracks: `hideTrack`, `unhideTrack` and `listHiddenTracks` (`api/openapi.yaml`). A
//! hidden track is gone from the caller's library and shown only in Hidden Songs, a page at a
//! time, newest first (`requirements/conventions.md` §8). Hiding a liked track unlikes it, as
//! liking a hidden one unhides it (`playlists::like`), so the two never contradict each other.
//! Each change is announced to the caller's devices once it is committed.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use jewelcase_catalog::hidden::{self as shared, DEFAULT_ORDER, HIDDEN_AT};
use jewelcase_catalog::mutate::Writer;
use serde::Deserialize;

use super::extract::{Path, Query};
use super::page::{Order, Page, Request};
use super::playlists;
use super::realtime::{Hub, ServerMessage};
use super::scope::Scope;
use super::tracks::TrackSummary;
use super::{Id, Problem};
use crate::db::{Database, now_ms};

/// The parameters `listHiddenTracks` supports. Any other answers `422` rather than being ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    order: Option<Order>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// `hideTrack`: hides the track from the caller in this library, and unlikes it. A track the
/// library does not have is not found; one hidden already stays as it was.
pub async fn hide(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    playlists::settle(async move {
        let (hid, unliked) = db
            .try_write(move |tx| {
                let mut effects = |effect| playlists::record(tx, user, library, effect);
                let mut w = Writer::new(tx, user, library, now_ms(), &mut effects);
                let (hid, removed) = w.hide(track)?;
                let unliked = removed
                    .map(|removed| playlists::removal(tx, user, library, removed))
                    .transpose()?;
                Ok::<_, Problem>((hid, unliked))
            })
            .await?;
        if unliked.is_some() {
            playlists::announce_liked(&hub, user, library, [track], false);
        }
        playlists::announce(&hub, user, unliked);
        if hid {
            announce(&hub, user, library, [track], true);
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `unhideTrack`: brings the track back into the caller's library. One not hidden is already
/// so, which is the answer.
pub async fn unhide(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    playlists::settle(async move {
        let unhid = db
            .write(move |tx| {
                let mut effects = |effect| playlists::record(tx, user, library, effect);
                Writer::new(tx, user, library, now_ms(), &mut effects).unhide(track)
            })
            .await?;
        if unhid {
            announce(&hub, user, library, [track], false);
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `listHiddenTracks`: Hidden Songs, the tracks the caller hid in this library, newest first
/// unless `order` says `asc`, a page at a time.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<TrackSummary>>, Problem> {
    let order = query.order.unwrap_or(DEFAULT_ORDER);
    let request = Request::new(&HIDDEN_AT, order, query.cursor, query.limit)?;
    let page = db.read(move |conn| shared::list(conn, user, library, request)).await?;
    Ok(Json(page))
}

/// Tells `user`'s devices that `tracks` of `library` were hidden, or unhidden. Call it once the
/// change is committed.
pub fn announce(
    hub: &Hub,
    user: i64,
    library: i64,
    tracks: impl IntoIterator<Item = i64>,
    hidden: bool,
) {
    for track in tracks {
        let message =
            ServerMessage::TrackHidden { library_id: Id(library), track_id: Id(track), hidden };
        hub.broadcast(user, &message);
    }
}

pub use jewelcase_catalog::hidden::{LibraryLoss, ids, losses};

#[cfg(test)]
mod tests;
