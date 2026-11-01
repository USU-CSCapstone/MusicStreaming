//! A playlist's items, `listPlaylistItems`, and `checkPlaylistDuplicates` (`api/openapi.yaml`).
//!
//! Both are scoped to the caller's own playlist in the library, so another account's, an
//! admin's included, is as absent as one that never was (`requirements/users.md` §10).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::api::cursor;
use crate::api::extract::{self, Path, Query};
use crate::api::playables::{self, Playable};
use crate::api::problem::Code;
use crate::api::scope::Scope;
use crate::api::time::serialize_rfc3339;
use crate::api::tracks::{self, TrackSummary};
use crate::api::{Id, Problem, page};
use crate::db::{Database, Json as JsonColumn, by_ids};

/// What names the list in its cursors.
const LABEL: &str = "playlists.items";

/// A playlist's items after a key, in order: one range of the primary key.
pub(super) const ITEMS: &str = "SELECT key, track_id, added_at FROM playlist_items \
                     WHERE playlist_id = ?1 AND key > ?2 ORDER BY key LIMIT ?3";

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

/// The spec's `PlaylistItem`; an item of an external track is not possible yet.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    track: TrackSummary,
    #[serde(serialize_with = "serialize_rfc3339")]
    added_at: i64,
}

/// The spec's `PlaylistItemPage`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemPage {
    items: Vec<Item>,
    next_cursor: Option<String>,
    total: i64,
    version: i64,
}

/// The version and item count of `user`'s playlist `id` in `library`, if it is theirs.
pub(super) fn header(
    conn: &Connection,
    user: i64,
    library: i64,
    id: i64,
) -> rusqlite::Result<Option<(i64, i64)>> {
    conn.prepare_cached(
        "SELECT version, track_count FROM playlists \
         WHERE id = ?1 AND user_id = ?2 AND library_id = ?3",
    )?
    .query_row(params![id, user, library], |row| Ok((row.get(0)?, row.get(1)?)))
    .optional()
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
    let limit = page::limit(query.limit)?;
    let after = query
        .cursor
        .map(|cursor| cursor::decode(&cursor, LABEL, 2))
        .transpose()?
        .map(|values| match values[..] {
            [Value::Integer(version), Value::Integer(key)] => Ok((version, key)),
            _ => Err(Problem::invalid("cursor is not valid for this list")),
        })
        .transpose()?;
    let page = db
        .try_read(move |conn| {
            let (version, total) =
                header(conn, user, library, id)?.ok_or_else(Problem::not_found)?;
            if after.is_some_and(|(cursor_version, _)| cursor_version != version) {
                return Err(Problem::new(Code::CursorExpired)
                    .detail("The playlist changed since the first page; start again."));
            }
            let start = after.map_or(i64::MIN, |(_, key)| key);
            // One more than the page, to know whether another follows. Keyset on the primary key.
            let mut rows: Vec<(i64, i64, i64)> = conn
                .prepare_cached(ITEMS)?
                .query_map(params![id, start, limit + 1], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<Result<_, _>>()?;
            let more = rows.len() > limit as usize;
            rows.truncate(limit as usize);
            let next_cursor = rows
                .last()
                .filter(|_| more)
                .map(|(key, ..)| cursor::encode(LABEL, &[version.into(), (*key).into()]));
            let mut wanted: Vec<i64> = rows.iter().map(|row| row.1).collect();
            wanted.sort_unstable();
            wanted.dedup();
            let summaries: HashMap<i64, TrackSummary> =
                by_ids(conn, tracks::SELECT, "tracks t", tracks::JOINS, library, &wanted, |row| {
                    let track = tracks::list_summary(row)?;
                    Ok((row.get::<_, i64>(0)?, track))
                })?
                .into_iter()
                .collect();
            // Every item's track is kept by retention, so none is left out in practice.
            let items = rows
                .iter()
                .filter_map(|(_, track, added_at)| {
                    let track = summaries.get(track)?.clone();
                    Some(Item { track, added_at: *added_at })
                })
                .collect();
            Ok::<_, Problem>(ItemPage { items, next_cursor, total, version })
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
