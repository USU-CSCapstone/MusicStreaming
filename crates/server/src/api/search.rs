//! Search: `search` and `searchSection` (`api/openapi.yaml`), over the core's indexes of names
//! (`jewelcase_core::Index`) and lyrics (`jewelcase_core::LyricIndex`, the section `lyrics`
//! shows), which each library keeps in memory (`indexes`). Names are the catalog's to search, as a
//! device does; lyrics are the server's alone (`design/search.md` §2.6).

mod indexes;
mod lyrics;
mod tagged;
mod within;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_catalog::cursor;
use jewelcase_catalog::page::{self, Order};
use jewelcase_catalog::search::{
    self, Hid, Keep, Lyrics, MUSIC, SEARCHED, Search, SearchResponse, SectionPage, SectionReading,
    SectionType, kinds, response, section_limit,
};
use jewelcase_catalog::within::Within;
use jewelcase_core::{Index, Kind, LyricHit, Union};
use lyrics::Ranked;
use rusqlite::types::Value;
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
    let (kinds, with_lyrics) = match query.types.as_deref() {
        None => (SEARCHED.to_vec(), true),
        Some(types) => names_and_lyrics(types)?,
    };
    let within = container(query.within.as_deref())?;

    let filter = TagFilter::new(query.tag_all.as_deref(), query.tag_any.as_deref())?;
    // Lyrics are tracks', so their matches are kept as tracks are.
    let mut kept = kinds.clone();
    if with_lyrics && !kept.contains(&Kind::Track) {
        kept.push(Kind::Track);
    }
    let tagged = indexes.tagged.get(&db, library, &filter, &kept).await?;
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
    let lyrics = if with_lyrics {
        let keep = Keep::new(&hid, tagged.as_deref(), held.as_deref());
        let keep = |id| keep.keeps(Kind::Track, id);
        Some(lyrics::ranked(&db, &indexes, library, &query.q, keep).await?)
    } else {
        None
    };
    let found_lyrics = lyrics.as_ref().is_some_and(|ranked| !ranked.hits.is_empty());
    // Found nothing, what was nearly found, of what the search could have found, and so as
    // narrowed and without what the caller hid (`requirements/search.md` §8). Matching the whole
    // vocabulary again takes a while, so it runs off the async threads, with the search.
    let (q, tag_type) = (query.q.clone(), query.tag_type.clone());
    let (found, misses, (hid, tagged, held)) = tokio::task::spawn_blocking(move || {
        let both = Union(&[&index, &mine]);
        let keep = Keep::new(&hid, tagged.as_deref(), held.as_deref());
        let found = both.search_within(&q, &kinds, limit, |kind, id| keep.keeps(kind, id));
        let found = search::of_tag_type(found, tag_type.as_deref());
        let misses = match found_lyrics {
            true => Vec::new(),
            false => search::misses(&both, &q, &kinds, &found, &keep),
        };
        (found, misses, (hid, tagged, held))
    })
    .await
    .map_err(|_| Problem::new(Code::Internal))?;
    let tag_type = query.tag_type;
    let response = db
        .read(move |conn| {
            let lyrics = match lyrics {
                Some(Ranked { mut hits, .. }) => {
                    let total = hits.len();
                    hits.truncate(limit);
                    Some(Lyrics { matches: lyrics::matches(conn, user, library, &hits)?, total })
                }
                None => None,
            };
            let keep = Keep::new(&hid, tagged.as_deref(), held.as_deref());
            let search = Search { user, library, q: query.q, keep };
            response(conn, search, found, &misses, tag_type.as_deref(), lyrics)
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
        SectionType::Lyrics => {
            return lyric_section(db, indexes, user, library, query).await.map(Json);
        }
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

/// The spec's `searchSection` of `lyrics`: in the section's ranking only, a page continuing after
/// the last match of the one before. Only the server searches lyrics.
async fn lyric_section(
    db: Arc<Database>,
    indexes: Arc<Indexes>,
    user: i64,
    library: i64,
    query: SectionQuery,
) -> Result<SectionPage, Problem> {
    if query.sort.is_some() || query.order.is_some() {
        return Err(Problem::invalid("this section has no sorts"));
    }
    let limit = page::limit(query.limit)?;
    let label = format!("search.{LYRICS}");
    let after = query.cursor.as_deref().map(|cursor| lyric_place(cursor, &label)).transpose()?;
    let within = container(query.within.as_deref())?;
    let filter = TagFilter::new(query.tag_all.as_deref(), query.tag_any.as_deref())?;
    let tagged = indexes.tagged.get(&db, library, &filter, &[Kind::Track]).await?;
    let held = held(&db, &indexes, user, library, within).await?;
    let hid = db.read(move |conn| Hid::read(conn, user, library, &filter)).await?;
    let held = reached(within, held, &hid)?;
    let keep = Keep::new(&hid, tagged.as_deref(), held.as_deref());
    let keep = |id| keep.keeps(Kind::Track, id);
    let ranked = lyrics::ranked(&db, &indexes, library, &query.q, keep).await?;
    let start = after.map_or(0, |place| ranked.after(&place));
    let end = ranked.hits.len().min(start + limit as usize);
    let next_cursor = (end < ranked.hits.len()).then(|| {
        let (rest, hit) = ranked.place(end - 1);
        cursor::encode(&label, &[Value::Integer(i64::from(rest)), Value::Blob(hit.to_bytes())])
    });
    let page = ranked.hits[start..end].to_vec();
    let matches = db.read(move |conn| lyrics::matches(conn, user, library, &page)).await?;
    Ok(SectionPage::lyrics(query.q, matches, next_cursor, ranked.hits.len()))
}

/// The place in the lyrics section a `cursor` continues after. A cursor of another section, or
/// none of ours, is `422 validation_failed`.
fn lyric_place(cursor: &str, label: &str) -> Result<lyrics::Place, Problem> {
    match cursor::decode(cursor, label, 2)?.as_slice() {
        [Value::Integer(rest @ (0 | 1)), Value::Blob(bytes)] => {
            LyricHit::from_bytes(bytes).map(|hit| (*rest == 1, hit))
        }
        _ => None,
    }
    .ok_or_else(|| Problem::invalid("cursor is not valid for this list"))
}

/// The spec's name for the lyrics section.
const LYRICS: &str = "lyrics";

/// The kinds of names to search for the comma-separated section `types`, and whether to search
/// lyrics, which the catalog's [`kinds`] does not, since only the server searches them.
fn names_and_lyrics(types: &str) -> Result<(Vec<Kind>, bool), Problem> {
    let names: Vec<&str> = types.split(',').filter(|section| *section != LYRICS).collect();
    let with_lyrics = names.len() < types.split(',').count();
    let kinds = if names.is_empty() { Vec::new() } else { kinds(&names.join(","))? };
    Ok((kinds, with_lyrics))
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
