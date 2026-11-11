//! A playlist's items, `listPlaylistItems`, and `checkPlaylistDuplicates` (`api/openapi.yaml`).
//!
//! Both are scoped to the caller's own playlist in the library, so another account's, an
//! admin's included, is as absent as one that never was (`requirements/users.md` §10).

use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_catalog::playlists::{self, ItemPage, header};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::api::extract::{self, Path, Query};
use crate::api::playables::{self, Playable};
use crate::api::scope::Scope;
use crate::api::{Id, Problem};
use crate::db::{Database, Json as JsonColumn};

#[cfg(test)]
pub(super) use jewelcase_catalog::playlists::ITEMS;

/// Which of the IDs in a JSON array the playlist holds, in the array's order, each found by
/// `playlist_items_by_track`.
pub(super) const HELD: &str = "SELECT ids.value FROM json_each(?3) AS ids WHERE EXISTS \
                    (SELECT 1 FROM playlist_items i WHERE i.library_id = ?1 \
                     AND i.track_id = ids.value AND i.playlist_id = ?2) ORDER BY ids.key";

/// The parameters `listPlaylistItems` supports. Any other answers `422`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}

/// One page of the playlist's items in the user's order, a duplicate as often as it occurs and
/// a missing track in place. The version check, the rows and the summaries come from one read,
/// so a page is of one version. A cursor from another version is `410 cursor_expired`.
pub async fn list_items(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
    Path((_, Id(id))): Path<(Id, Id)>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ItemPage>, Problem> {
    let page = db
        .try_read(move |conn| {
            playlists::items(conn, user, library, id, query.cursor.as_deref(), query.limit)
                .map_err(Problem::from)
        })
        .await?;
    Ok(Json(page))
}

/// The spec's `checkPlaylistDuplicates` request.
#[derive(Deserialize)]
pub struct DuplicatesQuery {
    items: Vec<Playable>,
}

/// The spec's `checkPlaylistDuplicates` response.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Duplicates {
    track_ids: Vec<Id>,
}

/// The tracks `items` expand to that the playlist already holds, once each in request order.
/// Looks each distinct track up by index, so a long playlist is not read through. A playable
/// the caller cannot add is `422` as when adding it.
pub async fn duplicates(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
    Path((_, Id(id))): Path<(Id, Id)>,
    extract::Json(body): extract::Json<DuplicatesQuery>,
) -> Result<Json<Duplicates>, Problem> {
    let track_ids = db
        .try_read(move |conn| {
            header(conn, user, library, id)?.ok_or_else(Problem::not_found)?;
            let mut seen = HashSet::new();
            let asked: Vec<i64> = playables::expand(conn, user, library, &body.items)?
                .into_iter()
                .filter(|track| seen.insert(*track))
                .collect();
            // Already in request order, since the IDs lead the join.
            let held = conn
                .prepare_cached(HELD)?
                .query_map(params![library, id, JsonColumn(&asked)], |row| Ok(Id(row.get(0)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, Problem>(held)
        })
        .await?;
    Ok(Json(Duplicates { track_ids }))
}
