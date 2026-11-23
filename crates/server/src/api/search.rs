//! Search: `search` and `searchSection` (`api/openapi.yaml`), over the core's index
//! (`jewelcase_core::Index`), which each library keeps in memory (`indexes`).

mod indexes;
mod tagged;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_catalog::search::{
    Hid, Keep, SearchResponse, SectionPage, SectionType, after, kinds, response, section_limit,
    section_page,
};
use jewelcase_core::Kind;
use serde::Deserialize;

pub use indexes::Indexes;

use super::extract::{Path, Query};
use super::scope::Scope;
use super::tag_filter::TagFilter;
use super::{Id, Problem, page};
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
    let keep = Keep::new(&hid, tagged.as_deref());
    let found = index.search_within(&query.q, &kinds, limit, |kind, id| keep.keeps(kind, id));
    let response = db.read(move |conn| response(conn, user, library, query.q, found, &hid)).await?;
    Ok(Json(response))
}

/// The spec's `searchSection` parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SectionQuery {
    q: String,
    cursor: Option<String>,
    limit: Option<u32>,
    tag_all: Option<String>,
    tag_any: Option<String>,
}

/// The spec's `searchSection`: one section of `search`, all of it, a page at a time and in the
/// same ranking. A page continues after the last match of the one before, so one read after the
/// index is rebuilt neither repeats a match nor passes over one still there.
pub async fn section(
    State(db): State<Arc<Database>>,
    State(indexes): State<Arc<Indexes>>,
    Scope { user, library }: Scope,
    Path((_, section)): Path<(Id, String)>,
    Query(query): Query<SectionQuery>,
) -> Result<Json<SectionPage>, Problem> {
    if query.q.is_empty() {
        return Err(Problem::invalid("q must not be empty"));
    }
    let limit = page::limit(query.limit)?;
    let kind = match SectionType::named(&section).ok_or_else(Problem::not_found)? {
        SectionType::Kind(kind) => Some(kind),
        SectionType::Playlists => None,
        SectionType::Lyrics => return Err(Problem::invalid("lyrics cannot be searched yet")),
    };
    let after = match kind {
        Some(kind) => after(query.cursor.as_deref(), kind)?,
        None => None,
    };

    let filter = TagFilter::new(query.tag_all.as_deref(), query.tag_any.as_deref())?;
    let kinds: Vec<Kind> = kind.into_iter().collect();
    let tagged = indexes.tagged.get(&db, library, &filter, &kinds).await?;
    let index = indexes.index(&db, library).await?;
    let tags = filter.clone();
    let hid = db.read(move |conn| Hid::read(conn, user, library, &tags)).await?;
    let keep = Keep::new(&hid, tagged.as_deref());
    let found = kind
        .map(|kind| {
            index.page(&query.q, kind, after.as_ref(), limit as usize, |kind, id| {
                keep.keeps(kind, id)
            })
        })
        .unwrap_or(jewelcase_core::SectionPage { ids: Vec::new(), total: 0, next: None });
    let page =
        db.read(move |conn| section_page(conn, user, library, query.q, kind, found, &hid)).await?;
    Ok(Json(page))
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
