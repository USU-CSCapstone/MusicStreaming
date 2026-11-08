//! Artists: `listArtists` and `getArtist` (`api/openapi.yaml`). The queries are the catalog's
//! (`jewelcase_catalog::artists`), which a device runs too.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
#[cfg(test)]
use jewelcase_catalog::artists::source;
use jewelcase_catalog::artists::{self, ArtistSort};
use serde::Deserialize;

pub use jewelcase_catalog::artists::{Artist, ArtistSummary, SELECT, TAGGED, summary};

use super::extract::{Path, Query};
use super::page::{Order, Page, Request};
use super::scope::Scope;
use super::tag_filter::TagFilter;
use super::{Id, Problem};
use crate::db::Database;

/// The parameters `listArtists` supports so far. Any other, including the spec's other filters,
/// answers `422` rather than being ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    sort: ArtistSort,
    #[serde(default)]
    order: Order,
    tag_all: Option<String>,
    tag_any: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// Every artist credited in the library, featured-only ones included
/// (`requirements/artists.md` §5), or those whose work carries the tags `tagAll` and
/// `tagAny` name.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<ArtistSummary>>, Problem> {
    let request = Request::new(query.sort.sort(), query.order, query.cursor, query.limit)?;
    let tags = TagFilter::new(query.tag_all.as_deref(), query.tag_any.as_deref())?;
    let page = db.read(move |conn| artists::list(conn, library, request, &tags)).await?;
    Ok(Json(page))
}

/// `getArtist`: the artist, or not found when the library has no such artist.
pub async fn get(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Path((_, Id(artist))): Path<(Id, Id)>,
) -> Result<Json<Artist>, Problem> {
    db.read(move |conn| artists::artist(conn, library, artist))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

#[cfg(test)]
mod tests;
