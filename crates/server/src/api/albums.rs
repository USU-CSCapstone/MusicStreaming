//! Albums: `listAlbums` and `getAlbum` (`api/openapi.yaml`). The queries are the catalog's
//! (`jewelcase_catalog::albums`), which a device runs too.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
#[cfg(test)]
use jewelcase_catalog::albums::source;
use jewelcase_catalog::albums::{self, AlbumSort};
use serde::Deserialize;

pub use jewelcase_catalog::albums::{Album, AlbumSummary};

use super::credit::ArtistCredit;
use super::extract::{Path, Query};
use super::page::{Order, Page, Request};
use super::scope::Scope;
use super::tag_filter::TagFilter;
use super::{AnyId, Id, Problem};
use crate::db::Database;

/// The parameters `listAlbums` supports so far. Any other, including the spec's other
/// filters, answers `422` rather than being ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    sort: AlbumSort,
    #[serde(default)]
    order: Order,
    artist_id: Option<AnyId>,
    #[serde(default)]
    artist_credit: ArtistCredit,
    tag_all: Option<String>,
    tag_any: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// The library's albums, or those credited to `artistId` as `artistCredit` says and whose
/// tracks carry the tags `tagAll` and `tagAny` name, a page at a time.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<AlbumSummary>>, Problem> {
    let request = Request::new(query.sort.sort(), query.order, query.cursor, query.limit)?;
    let (artist, credit) = (query.artist_id, query.artist_credit);
    let tags = TagFilter::new(query.tag_all.as_deref(), query.tag_any.as_deref())?;
    let page =
        db.read(move |conn| albums::list(conn, library, request, artist, credit, &tags)).await?;
    Ok(Json(page))
}

/// `getAlbum`: the album, or not found when the library has no such album.
pub async fn get(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Path((_, Id(album))): Path<(Id, Id)>,
) -> Result<Json<Album>, Problem> {
    db.read(move |conn| albums::album(conn, library, album))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

#[cfg(test)]
mod tests;
