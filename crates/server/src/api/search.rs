//! Search: `search` (`api/openapi.yaml`), over the core's index (`jewelcase_core::Index`),
//! which each library keeps in memory (`indexes`).

mod indexes;
mod tagged;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_catalog::search::{Hid, SearchResponse, kinds, response, section_limit};
use jewelcase_core::Kind;
use serde::Deserialize;

pub use indexes::Indexes;

use super::Problem;
use super::extract::Query;
use super::scope::Scope;
use super::tag_filter::TagFilter;
use crate::db::Database;

/// The spec's `search` parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchQuery {
    q: String,
    /// Comma-separated section types.
    types: Option<String>,
    tag_all: Option<String>,
    tag_any: Option<String>,
    section_limit: Option<usize>,
}

/// The spec's `search`: each section's best matches for `q` in `library`, of those carrying the
/// tags `tagAll` and `tagAny` name.
pub async fn search(
    State(db): State<Arc<Database>>,
    State(indexes): State<Arc<Indexes>>,
    Scope { user, library }: Scope,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, Problem> {
    let limit = section_limit(&query.q, query.section_limit)?;
    let kinds = query.types.as_deref().map_or_else(|| Ok(Kind::ALL.to_vec()), kinds)?;

    let filter = TagFilter::new(query.tag_all.as_deref(), query.tag_any.as_deref())?;
    let tagged = indexes.tagged.get(&db, library, &filter, &kinds).await?;
    let index = indexes.index(&db, library).await?;
    // What the caller hid is gone from their library, so a search finds it nowhere.
    let tags = filter.clone();
    let hid = db.read(move |conn| Hid::read(conn, user, library, &tags)).await?;
    let found = index.search_within(&query.q, &kinds, limit, |kind, id| {
        !hid.contains(kind, id)
            && tagged.as_ref().is_none_or(|tagged| tagged[kind as usize].contains(&id))
    });
    let response = db.read(move |conn| response(conn, user, library, query.q, found, &hid)).await?;
    Ok(Json(response))
}

/// How many of `library`'s tracks, albums, and artists match `q`, as a search counts them, in
/// the order of `Kind::ALL`.
pub async fn totals(
    db: &Arc<Database>,
    indexes: &Indexes,
    library: i64,
    q: &str,
) -> Result<[usize; 3], Problem> {
    let index = indexes.index(db, library).await?;
    let found = index.search(q, &Kind::ALL, 1);
    Ok(Kind::ALL.map(|kind| {
        found.iter().find(|section| section.kind == kind).map_or(0, |section| section.total)
    }))
}

#[cfg(test)]
mod tests;
