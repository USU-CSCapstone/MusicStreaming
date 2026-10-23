//! Artists: `listArtists` and `getArtist` (`api/openapi.yaml`).

mod representation;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde::Deserialize;

pub use representation::{Artist, ArtistSummary, SELECT, summary};

use super::extract::{Path, Query};
use super::page::{Order, Page, Request, Sort, Table, Unknown};
use super::scope::Scope;
use super::{Id, Problem};
use crate::db::Database;

/// The `sort` values `listArtists` supports.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ArtistSort {
    #[default]
    Name,
    DateAdded,
    // The spec's `playCount` and `lastPlayed` are not implemented, so they answer `422`.
}

impl ArtistSort {
    /// The keyset this sort pages by. Each one's columns are one of `artists`' indexes after
    /// `library_id`.
    fn sort(self) -> &'static Sort {
        match self {
            ArtistSort::Name => &Sort {
                label: "artists.name",
                columns: &["ar.sort_key", "ar.id"],
                unknown: Unknown::Empty,
            },
            ArtistSort::DateAdded => &Sort {
                label: "artists.dateAdded",
                columns: &["ar.added_at", "ar.id"],
                unknown: Unknown::Never,
            },
        }
    }
}

/// The parameters `listArtists` supports so far. Any other, including the spec's filters,
/// answers `422` rather than being ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    sort: ArtistSort,
    #[serde(default)]
    order: Order,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// Every artist credited in the library, featured-only ones included
/// (`requirements/artists.md` §5).
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<ArtistSummary>>, Problem> {
    let request = Request::new(query.sort.sort(), query.order, query.cursor, query.limit)?;
    let page = db
        .read(move |conn| {
            let source = ARTISTS.source(conn, library, None, None)?;
            request.read(conn, &source, summary)
        })
        .await?;
    Ok(Json(page))
}

/// `getArtist`: the artist, or not found when the library has no such artist.
pub async fn get(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Path((_, Id(artist))): Path<(Id, Id)>,
) -> Result<Json<Artist>, Problem> {
    db.read(move |conn| representation::artist(conn, library, artist))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

/// The artists a list reads.
const ARTISTS: Table =
    Table { name: "artists", alias: "ar", select: SELECT, joins: "", count: "artist_count" };

#[cfg(test)]
mod tests;
