//! Artists: `listArtists` and `getArtist` (`api/openapi.yaml`).

mod representation;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde::Deserialize;

pub use representation::{Artist, ArtistSummary, SELECT, summary};

use super::extract::{Path, Query};
use super::page::{Order, Page, Request, Sort, Source, Unknown};
use super::{Id, Problem};
use crate::db::Database;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ArtistSort {
    #[default]
    Name,
    DateAdded,
    // `playCount` and `lastPlayed` wait for listening history, and answer `422` until then.
}

impl ArtistSort {
    /// Each is one of `artists`' indexes after `library_id`.
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
    Path(Id(library)): Path<Id>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<ArtistSummary>>, Problem> {
    let request = Request::new(query.sort.sort(), query.order, query.cursor, query.limit)?;
    let source = source(library);
    db.read(move |conn| request.read(conn, library, "artist_count", false, &source, summary))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

pub async fn get(
    State(db): State<Arc<Database>>,
    Path((Id(library), Id(artist))): Path<(Id, Id)>,
) -> Result<Json<Artist>, Problem> {
    db.read(move |conn| representation::artist(conn, library, artist))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

fn source(library: i64) -> Source {
    Source {
        select: SELECT,
        from: "artists ar".to_owned(),
        filter: "ar.library_id = ?".to_owned(),
        params: vec![library.into()],
    }
}

#[cfg(test)]
mod tests;
