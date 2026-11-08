//! Search: `search` (`api/openapi.yaml`), over the core's index (`jewelcase_core::Index`),
//! which each library keeps in memory (`indexes`).

mod indexes;
mod tagged;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_catalog::search::{SearchResponse, kinds, response};
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
    Scope { library, .. }: Scope,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, Problem> {
    let invalid = |detail| Problem::invalid(detail);
    if query.q.is_empty() {
        return Err(invalid("q must not be empty"));
    }
    let limit = query.section_limit.unwrap_or(5);
    if !(1..=50).contains(&limit) {
        return Err(invalid("sectionLimit must be from 1 to 50"));
    }
    let kinds = query.types.as_deref().map_or_else(|| Ok(Kind::ALL.to_vec()), kinds)?;

    let filter = TagFilter::new(query.tag_all.as_deref(), query.tag_any.as_deref())?;
    let tagged = indexes.tagged.get(&db, library, &filter, &kinds).await?;
    let index = indexes.index(&db, library).await?;
    let found = index.search_within(&query.q, &kinds, limit, |kind, id| {
        tagged.as_ref().is_none_or(|tagged| tagged[kind as usize].contains(&id))
    });
    let response = db.read(move |conn| response(conn, library, query.q, found)).await?;
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
