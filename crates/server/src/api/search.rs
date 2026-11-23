//! Search: `search` and `searchSection` (`api/openapi.yaml`), over the core's index
//! (`jewelcase_core::Index`), which each library keeps in memory (`indexes`).

mod indexes;
mod tagged;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_catalog::page::Order;
use jewelcase_catalog::search::{
    self, Hid, Keep, MUSIC, SEARCHED, Search, SearchResponse, SectionPage, SectionReading,
    SectionType, kinds, response, section_limit,
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
    /// Only tags of this type, of the tags sections.
    tag_type: Option<String>,
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
    let kinds = query.types.as_deref().map_or_else(|| Ok(SEARCHED.to_vec()), kinds)?;

    let filter = TagFilter::new(query.tag_all.as_deref(), query.tag_any.as_deref())?;
    let tagged = indexes.tagged.get(&db, library, &filter, &kinds).await?;
    let index = indexes.index(&db, library).await?;
    // What the caller hid is gone from their library, so a search finds it nowhere.
    let tags = filter.clone();
    let hid = db.read(move |conn| Hid::read(conn, user, library, &tags)).await?;
    let keep = Keep::new(&hid, tagged.as_deref());
    let found = index.search_within(&query.q, &kinds, limit, |kind, id| keep.keeps(kind, id));
    let found = search::of_tag_type(found, query.tag_type.as_deref());
    let response = db.read(move |conn| response(conn, user, library, query.q, found, &hid)).await?;
    Ok(Json(response))
}

/// The spec's `searchSection` parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SectionQuery {
    q: String,
    /// The type of tag of the tags section.
    tag_type: Option<String>,
    sort: Option<String>,
    order: Option<Order>,
    cursor: Option<String>,
    limit: Option<u32>,
    tag_all: Option<String>,
    tag_any: Option<String>,
}

/// The spec's `searchSection`: one section of `search`, all of it, a page at a time, in the
/// same ranking or by one of the section's list sorts. A ranked page continues after the last
/// match of the one before, so one read after the index is rebuilt neither repeats a match nor
/// passes over one still there; a sorted page continues as its list's do.
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
    let kind = match SectionType::named(&section).ok_or_else(Problem::not_found)? {
        SectionType::Kind(kind) => kind,
        SectionType::Playlists => {
            page::limit(query.limit)?;
            return Ok(Json(SectionPage::empty(query.q)));
        }
        SectionType::Lyrics => return Err(Problem::invalid("lyrics cannot be searched yet")),
    };
    let SectionQuery { q, tag_type, sort, order, cursor, limit, tag_all, tag_any } = query;
    // Each type of tag is a section of its own, so the tags section is one type.
    let group = match kind {
        Kind::Tag => Some(tag_type.ok_or_else(|| Problem::invalid("tags need a tagType"))?),
        _ => None,
    };
    let reading = SectionReading::new(kind, sort.as_deref(), order, cursor, limit)?;

    let filter = TagFilter::new(tag_all.as_deref(), tag_any.as_deref())?;
    let tagged = indexes.tagged.get(&db, library, &filter, &[kind]).await?;
    let index = indexes.index(&db, library).await?;
    let page = db
        .read(move |conn| {
            let hid = Hid::read(conn, user, library, &filter)?;
            let keep = Keep::new(&hid, tagged.as_deref());
            let group = group.as_deref();
            search::section(conn, Search { user, library, q, keep }, kind, group, reading, &index)
        })
        .await?;
    Ok(Json(page))
}

/// How many of `library`'s tracks, albums, and artists match `q`, as a search counts them, in
/// that order: what a plugin sent a user's searches is told they found.
pub async fn totals(
    db: &Arc<Database>,
    indexes: &Indexes,
    library: i64,
    q: &str,
) -> Result<[usize; 3], Problem> {
    let index = indexes.index(db, library).await?;
    let found = index.search(q, &MUSIC, 1);
    Ok(MUSIC.map(|kind| {
        found.iter().find(|section| section.kind == kind).map_or(0, |section| section.total)
    }))
}

#[cfg(test)]
mod tests;
