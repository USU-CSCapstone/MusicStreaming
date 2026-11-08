//! Tracks: `listTracks` and `getTrack` (`api/openapi.yaml`). The queries are the catalog's
//! (`jewelcase_catalog::tracks`), which a device runs too.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_catalog::tracks;
use serde::Deserialize;

#[cfg(test)]
pub use jewelcase_catalog::tracks::TRACKS;
pub use jewelcase_catalog::tracks::{
    CREDITS, FEATURED_TRACKS, JOINS, LOUDNESS, Loudness, SELECT, TAGGED, TRACK_NUMBER, Track,
    TrackOrder, TrackSort, TrackSummary, list_summary, loudness, resolve, sort_keys,
};
#[cfg(test)]
use jewelcase_catalog::tracks::{WITHIN_ALBUM, source};

use super::credit::ArtistCredit;
use super::extract::{Path, Query};
use super::page::{Order, Page, Request};
use super::scope::Scope;
#[cfg(test)]
use super::tag_filter::LEAD_SHARE;
use super::tag_filter::TagFilter;
use super::{AnyId, Id, Problem};
use crate::db::Database;

/// The most `ids` one request may name: a default page.
const MAX_IDS: usize = super::page::DEFAULT_LIMIT as usize;

/// The parameters `listTracks` supports so far. Any other, including the spec's other
/// filters, answers `422` rather than being ignored. The ones with defaults are optional so
/// that `ids`, which excludes every other parameter, can tell they were not sent.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    sort: Option<TrackSort>,
    order: Option<Order>,
    album_id: Option<AnyId>,
    artist_id: Option<AnyId>,
    artist_credit: Option<ArtistCredit>,
    tag_all: Option<String>,
    tag_any: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
    /// Comma-separated track IDs, as the spec's `ids` says.
    ids: Option<String>,
}

impl ListQuery {
    /// Whether any parameter but `ids` was sent.
    fn has_other_parameters(&self) -> bool {
        self.sort.is_some()
            || self.order.is_some()
            || self.album_id.is_some()
            || self.artist_id.is_some()
            || self.artist_credit.is_some()
            || self.tag_all.is_some()
            || self.tag_any.is_some()
            || self.cursor.is_some()
            || self.limit.is_some()
    }
}

/// The tracks `ids` names that are in `library`, in the order named, as one page. An ID that
/// is malformed, unknown, or in another library is left out, so the answer never says which.
/// More than [`MAX_IDS`] IDs, or any parameter besides `ids`, is `422`.
async fn by_ids(
    db: Arc<Database>,
    library: i64,
    query: ListQuery,
) -> Result<Json<Page<TrackSummary>>, Problem> {
    if query.has_other_parameters() {
        return Err(Problem::invalid("ids cannot be combined with other parameters"));
    }
    let text = query.ids.unwrap_or_default();
    let ids: Vec<i64> = text.split(',').filter_map(|id| AnyId::from(id).0).collect();
    if text.split(',').count() > MAX_IDS {
        return Err(Problem::invalid(format!("ids names at most {MAX_IDS} tracks")));
    }
    Ok(Json(db.read(move |conn| tracks::by_ids(conn, library, &ids)).await?))
}

/// The library's tracks, or those on `albumId`, credited to `artistId` as `artistCredit` says,
/// and carrying the tags `tagAll` and `tagAny` name, a page at a time; or, given `ids`, just
/// those tracks (see [`by_ids`]).
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<TrackSummary>>, Problem> {
    if query.ids.is_some() {
        return by_ids(db, library, query).await;
    }
    let order =
        TrackOrder { sort: query.sort.unwrap_or_default(), order: query.order.unwrap_or_default() };
    let (sort, order) = order.keyset(query.album_id.is_some());
    let request = Request::new(sort, order, query.cursor, query.limit)?;
    let credit = query.artist_credit.unwrap_or_default();
    let (album, artist) = (query.album_id, query.artist_id);
    let tags = TagFilter::new(query.tag_all.as_deref(), query.tag_any.as_deref())?;
    let page = db
        .read(move |conn| tracks::list(conn, library, request, album, artist, credit, &tags))
        .await?;
    Ok(Json(page))
}

/// `getTrack`: the track, or not found when the library has no such track.
pub async fn get(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
) -> Result<Json<Track>, Problem> {
    db.read(move |conn| tracks::track(conn, library, track))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

#[cfg(test)]
mod tests;
