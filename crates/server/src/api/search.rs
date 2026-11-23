//! Search: `search` and `searchSection` (`api/openapi.yaml`), over the core's index
//! (`jewelcase_core::Index`), which each library keeps in memory (`indexes`).

mod indexes;
mod tagged;
mod within;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_catalog::page::Order;
use jewelcase_catalog::search::{
    self, Hid, Keep, MUSIC, SEARCHED, Search, SearchResponse, SectionPage, SectionReading,
    SectionType, kinds, response, section_limit,
};
use jewelcase_catalog::within::Within;
use jewelcase_core::{Index, Kind, Union};
use serde::Deserialize;

pub use indexes::Indexes;
use tagged::Sets;

use super::extract::{Path, Query};
use super::scope::Scope;
use super::tag_filter::TagFilter;
use super::{Code, Id, Problem};
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
    within: Option<String>,
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
    let within = container(query.within.as_deref())?;

    let filter = TagFilter::new(query.tag_all.as_deref(), query.tag_any.as_deref())?;
    let tagged = indexes.tagged.get(&db, library, &filter, &kinds).await?;
    let index = indexes.index(&db, library).await?;
    // What the caller hid is gone from their library, so a search finds it nowhere.
    // And the caller's own playlists, which no one else's search finds.
    let tags = filter.clone();
    let wants = kinds.contains(&Kind::Playlist);
    // Narrowed to a container, only what it holds, kept as a tag filter's rows are.
    let held = held(&db, &indexes, user, library, within).await?;
    let (hid, mine) = db
        .read(move |conn| {
            let mine = if wants { search::mine(conn, user, library)? } else { Index::default() };
            Ok::<_, rusqlite::Error>((Hid::read(conn, user, library, &tags)?, mine))
        })
        .await?;
    let held = reached(within, held, &hid)?;
    // Found nothing, what was nearly found, of what the search could have found, and so as
    // narrowed and without what the caller hid (`requirements/search.md` §8). Matching the whole
    // vocabulary again takes a while, so it runs off the async threads, with the search.
    let (q, tag_type) = (query.q.clone(), query.tag_type.clone());
    let (found, misses, (hid, tagged, held)) = tokio::task::spawn_blocking(move || {
        let both = Union(&[&index, &mine]);
        let keep = Keep::new(&hid, tagged.as_deref(), held.as_deref());
        let found = both.search_within(&q, &kinds, limit, |kind, id| keep.keeps(kind, id));
        let found = search::of_tag_type(found, tag_type.as_deref());
        let misses = search::misses(&both, &q, &kinds, &found, &keep);
        (found, misses, (hid, tagged, held))
    })
    .await
    .map_err(|_| Problem::new(Code::Internal))?;
    let tag_type = query.tag_type;
    let response = db
        .read(move |conn| {
            let keep = Keep::new(&hid, tagged.as_deref(), held.as_deref());
            let search = Search { user, library, q: query.q, keep };
            response(conn, search, found, &misses, tag_type.as_deref())
        })
        .await?;
    Ok(Json(response))
}

/// The spec's `searchSection` parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SectionQuery {
    q: String,
    /// The type of tag of the tags section.
    tag_type: Option<String>,
    within: Option<String>,
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
        SectionType::Lyrics => return Err(Problem::invalid("lyrics cannot be searched yet")),
    };
    let SectionQuery { q, tag_type, within, sort, order, cursor, limit, tag_all, tag_any } = query;
    let within = container(within.as_deref())?;
    // Each type of tag is a section of its own, so the tags section is one type.
    let group = match kind {
        Kind::Tag => Some(tag_type.ok_or_else(|| Problem::invalid("tags need a tagType"))?),
        _ => None,
    };
    let reading = SectionReading::new(kind, sort.as_deref(), order, cursor, limit)?;

    let filter = TagFilter::new(tag_all.as_deref(), tag_any.as_deref())?;
    let tagged = indexes.tagged.get(&db, library, &filter, &[kind]).await?;
    let index = indexes.index(&db, library).await?;
    let held = held(&db, &indexes, user, library, within).await?;
    let page = db
        .read(move |conn| {
            let hid = Hid::read(conn, user, library, &filter)?;
            // Narrowed to a container, only what it holds; one out of reach is not found.
            let Ok(held) = reached(within, held, &hid) else { return Ok(None) };
            let keep = Keep::new(&hid, tagged.as_deref(), held.as_deref());
            let mine = if kind == Kind::Playlist {
                search::mine(conn, user, library)?
            } else {
                Index::default()
            };
            let both = Union(&[&index, &mine]);
            let group = group.as_deref();
            let search = Search { user, library, q, keep };
            search::section(conn, search, kind, group, reading, &both).map(Some)
        })
        .await?;
    page.map(Json).ok_or_else(Problem::not_found)
}

/// The container `within` names, if any: one that names nothing there could be is `404`, as one
/// out of the caller's reach is.
fn container(within: Option<&str>) -> Result<Option<Within>, Problem> {
    within.map(|within| Within::parse(within)?.ok_or_else(Problem::not_found)).transpose()
}

/// What the container `within` holds in `library` as `user` reaches it, if a search is narrowed
/// to one, kept while the library stays as it is ([`Containers`](within::Containers)).
async fn held(
    db: &Database,
    indexes: &Indexes,
    user: i64,
    library: i64,
    within: Option<Within>,
) -> Result<Option<Option<Sets>>, Problem> {
    match within {
        Some(within) => Ok(Some(indexes.containers.get(db, user, library, within).await?)),
        None => Ok(None),
    }
}

/// What a container holds, if the search is narrowed to one: `404` where it is out of reach, as
/// one not there is, or one the caller hid whole, as `hid` says (`requirements/users.md` §10).
fn reached(
    within: Option<Within>,
    held: Option<Option<Sets>>,
    hid: &Hid,
) -> Result<Option<Sets>, Problem> {
    match (within, held) {
        (Some(within), Some(Some(held))) if !hid.contains(within.kind(), within.id()) => {
            Ok(Some(held))
        }
        (Some(_), _) => Err(Problem::not_found()),
        (None, _) => Ok(None),
    }
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
