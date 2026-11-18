//! Search: the documents a library's search index is built from, and the responses a search
//! gives (`search` and `searchSection`, `api/openapi.yaml`). The index itself is the core's
//! (`jewelcase_core::Index`), which the server keeps in memory and a device builds from the same
//! documents.

use std::collections::HashSet;

use jewelcase_core::{
    Document, Index, IndexBuilder, Kind, Rank, Score, Section as Found, SectionPage as FoundPage,
};
use rusqlite::types::Value;
use rusqlite::{Connection, Row, params};
use serde::de::IntoDeserializer;
use serde::de::value::StrDeserializer;
use serde::{Deserialize, Serialize};

use crate::Invalid;
use crate::albums::{self, AlbumSort, AlbumSummary};
use crate::artists::{self, ArtistSort, ArtistSummary};
use crate::by_ids::by_ids;
use crate::cursor;
use crate::hidden::{self, Losses};
use crate::page::{
    self, IdWalk, Lead, Order, Page, Request, Sort, Source, Table, Total, Walk, library_number,
};
use crate::tag_filter::{LEAD_SHARE, TagFilter, Tagged, Tags};
use crate::tracks::{self, TrackSort, TrackSummary};

/// The kinds a search finds so far: playlists and tags are not indexed yet, so their sections are
/// always empty.
pub const SEARCHED: [Kind; 3] = [Kind::Track, Kind::Album, Kind::Artist];

/// The spec's `SearchResponse`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    query: String,
    top: Option<SearchResult>,
    sections: Vec<Section>,
    near_misses: Vec<String>,
    library_empty: bool,
}

/// One of a `SearchResponse`'s `sections`.
#[derive(Serialize)]
struct Section {
    #[serde(rename = "type")]
    kind: &'static str,
    items: Vec<SearchResult>,
    total: usize,
}

/// The spec's `SearchSectionPage`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionPage {
    query: String,
    items: Vec<SearchResult>,
    next_cursor: Option<String>,
    total: usize,
}

/// What a `searchSection` names: one kind's matches, or a section that has none yet.
pub enum SectionType {
    Kind(Kind),
    /// The caller's playlists, which the index does not hold yet: an empty page.
    Playlists,
    /// Lyrics, which are not indexed yet: [`Invalid`].
    Lyrics,
}

impl SectionType {
    /// The section `name` names, as the spec's `SearchSectionType` spells it, if any.
    pub fn named(name: &str) -> Option<SectionType> {
        match name {
            "playlists" => Some(SectionType::Playlists),
            "lyrics" => Some(SectionType::Lyrics),
            name => {
                SEARCHED.into_iter().find(|&kind| section_type(kind) == name).map(SectionType::Kind)
            }
        }
    }
}

/// How a section is read: in the search's ranking, `limit` matches continuing after a match's
/// rank, or as its list sorts it.
pub enum SectionReading<'a> {
    Ranked { after: Option<Rank>, limit: u32 },
    Sorted(Request<'a>),
}

impl SectionReading<'_> {
    /// How the section of `kind` is read for a `sort` and `order` (the list's, or the
    /// ranking without them), continuing after `cursor`, `limit` matches at a time. A sort the
    /// section's list does not take, an `order` with no sort, and a cursor of another section or
    /// sort, are [`Invalid`].
    pub fn new(
        kind: Kind,
        sort: Option<&str>,
        order: Option<Order>,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<Self, Invalid> {
        match sort {
            None if order.is_some() => Err(Invalid("order needs a sort".to_owned())),
            None => {
                let limit = page::limit(limit)?;
                Ok(SectionReading::Ranked { after: after(cursor.as_deref(), kind)?, limit })
            }
            Some(sort) => {
                let order = order.unwrap_or_default();
                Ok(SectionReading::Sorted(Request::new(
                    list_sort(kind, sort)?,
                    order,
                    cursor,
                    limit,
                )?))
            }
        }
    }
}

/// The rank a page of `kind`'s section continues after, from its `cursor`: a cursor of another
/// section, or none of ours, is [`Invalid`].
fn after(cursor: Option<&str>, kind: Kind) -> Result<Option<Rank>, Invalid> {
    let Some(cursor) = cursor else { return Ok(None) };
    rank(&cursor::decode(cursor, &label(kind), RANK_VALUES)?)
        .map(Some)
        .ok_or_else(|| Invalid("cursor is not valid for this list".to_owned()))
}

/// How many values a rank is written as in a cursor.
const RANK_VALUES: usize = 7;

/// `rank` as a cursor's values: its score's, its name's words between spaces, which no folded
/// word holds, then its ID.
fn rank_values(rank: &Rank) -> [Value; RANK_VALUES] {
    let Score { inexact, typos, skipped, prefixes, extra_words } = rank.score;
    [
        Value::Integer(inexact.into()),
        Value::Integer(typos.into()),
        Value::Integer(skipped.into()),
        Value::Integer(prefixes.into()),
        Value::Integer(extra_words.into()),
        Value::Text(rank.words.join(" ")),
        Value::Integer(rank.id),
    ]
}

/// The rank [`rank_values`] wrote, or `None` for values it could not have.
fn rank(values: &[Value]) -> Option<Rank> {
    let flag = |value: &Value| match value {
        Value::Integer(0) => Some(false),
        Value::Integer(1) => Some(true),
        _ => None,
    };
    let count = |value: &Value| match value {
        Value::Integer(n) => u32::try_from(*n).ok(),
        _ => None,
    };
    let [inexact, typos, skipped, prefixes, extra_words, Value::Text(words), Value::Integer(id)] =
        values
    else {
        return None;
    };
    Some(Rank {
        score: Score {
            inexact: flag(inexact)?,
            typos: count(typos)?,
            skipped: flag(skipped)?,
            prefixes: count(prefixes)?,
            extra_words: count(extra_words)?,
        },
        words: words.split(' ').filter(|word| !word.is_empty()).map(str::to_owned).collect(),
        id: *id,
    })
}

/// The label of `kind`'s section's cursors, so one is only accepted where it came from.
fn label(kind: Kind) -> String {
    format!("search.{}", section_type(kind))
}

/// The keyset of the list sort `name` of the section of `kind`: one of `listTracks`',
/// `listAlbums`', or `listArtists`'. Any other is [`Invalid`].
fn list_sort(kind: Kind, name: &str) -> Result<&'static Sort, Invalid> {
    let name: StrDeserializer<serde::de::value::Error> = name.into_deserializer();
    match kind {
        Kind::Track => TrackSort::deserialize(name).map(TrackSort::sort),
        Kind::Album => AlbumSort::deserialize(name).map(AlbumSort::sort),
        Kind::Artist => ArtistSort::deserialize(name).map(ArtistSort::sort),
        // Neither has a list of its own to sort as.
        Kind::Playlist | Kind::Tag => {
            return Err(Invalid("this section has no sorts".to_owned()));
        }
    }
    .map_err(|_| Invalid(format!("sort must be one the {} list takes", section_type(kind))))
}

impl SectionPage {
    /// The page of a section that has nothing in it yet, for a search for `q`.
    pub fn empty(q: String) -> SectionPage {
        SectionPage { query: q, items: Vec::new(), next_cursor: None, total: 0 }
    }
}

/// A search: who searches, in which library, for what, and what it shows them. The server and a
/// device each make one for a search's response or one of its sections.
pub struct Search<'a> {
    pub user: i64,
    pub library: i64,
    pub q: String,
    pub keep: Keep<'a>,
}

/// A page of the section of `kind` of `search` with `index`, read as `reading` says, as the
/// browse lists show it to the searcher, with the cursor to the next.
pub fn section(
    conn: &Connection,
    search: Search,
    kind: Kind,
    reading: SectionReading,
    index: &Index,
) -> rusqlite::Result<SectionPage> {
    let Search { user, library, q, keep } = search;
    let losses = &keep.hid.losses;
    match reading {
        SectionReading::Ranked { after, limit } => {
            let found = index.page(&q, kind, None, after.as_ref(), limit as usize, |kind, id| {
                keep.keeps(kind, id)
            });
            let label = label(kind);
            let next_cursor = found.next.map(|rank| cursor::encode(&label, &rank_values(&rank)));
            let FoundPage { ids, total, .. } = found;
            let found = vec![Found { kind, group: None, ids, total }];
            let items = sections(conn, user, library, found, losses)?
                .pop()
                .map_or_else(Vec::new, |section| section.items);
            Ok(SectionPage { query: q, items, next_cursor, total })
        }
        SectionReading::Sorted(request) => {
            // Every match, unranked: the sort orders them.
            let ids = index.matching(&q, kind, |kind, id| keep.keeps(kind, id));
            let page = sorted(conn, user, library, kind, request, &ids, losses)?;
            let total = usize::try_from(page.total).unwrap_or(0);
            Ok(SectionPage { query: q, items: page.items, next_cursor: page.next_cursor, total })
        }
    }
}

/// The page `request` asks for of the matches `ids` of `kind`, sorted as their list sorts
/// them, as `user` sees them, whose hidden tracks take `losses` from albums and artists. The
/// matches are already only what the search keeps. Only their IDs and sort keys are read, their
/// summaries after, by ID, as a ranked section's are.
fn sorted(
    conn: &Connection,
    user: i64,
    library: i64,
    kind: Kind,
    request: Request,
    ids: &[i64],
    losses: &Losses,
) -> rusqlite::Result<Page<SearchResult>> {
    let (table, id): (&Table, &'static [&'static str]) = match kind {
        Kind::Track => (&tracks::TRACKS, &["t.id"]),
        Kind::Album => (&albums::ALBUMS, &["al.id"]),
        Kind::Artist => (&artists::ARTISTS, &["ar.id"]),
        // `list_sort` refuses a sort of either.
        Kind::Playlist | Kind::Tag => {
            return Ok(Page { items: Vec::new(), next_cursor: None, total: 0 });
        }
    };
    let page = read_matches(conn, library, table, id, request, ids)?;
    let found = Found { kind, group: None, ids: page.items, total: ids.len() };
    let items = sections(conn, user, library, vec![found], losses)?.pop();
    let items = items.map_or_else(Vec::new, |section| section.items);
    Ok(Page { items, next_cursor: page.next_cursor, total: page.total })
}

/// The fewest rows a sorted section's walk reads before it gives up and leads: a few
/// milliseconds' worth, which leading a few thousand matches costs too.
const MIN_WALK: u32 = 10_000;

/// The IDs of the page `request` asks for of the rows of `table` that `ids` names, whose `id`
/// column is the one selected (`design/database.md` §4). Only IDs and sort keys are read, so rows
/// passed over cost little. Many of the table's rows, more than one in [`LEAD_SHARE`], walk the
/// sort's index, keeping those named ([`IdWalk`]), which reads about a page's worth over the share
/// named where they are spread through the sort. Few lead the page, as one JSON array, which
/// sorts them all and so costs what they are; and so does a walk that has read as many rows as
/// there are named, or [`MIN_WALK`] if more, without filling the page, since those named bunch
/// together in the sort, so a page costs at most about twice what leading would.
fn read_matches(
    conn: &Connection,
    library: i64,
    table: &'static Table,
    id: &'static [&'static str],
    request: Request,
    ids: &[i64],
) -> rusqlite::Result<Page<i64>> {
    let total = i64::try_from(ids.len()).unwrap_or(i64::MAX);
    let rows = library_number(conn, library, table.count)?;
    let id_of = |row: &Row| row.get::<_, i64>(0);
    if total.saturating_mul(LEAD_SHARE) <= rows {
        return request.read(conn, &led(conn, library, table, id, ids)?, id_of);
    }
    let named: HashSet<i64> = ids.iter().copied().collect();
    let ids = ids.to_vec();
    let walk = IdWalk {
        keeps: Box::new(move |id| named.contains(&id)),
        // More than none, or they would lead.
        spread: u32::try_from((rows + total - 1) / total).unwrap_or(u32::MAX),
        budget: u32::try_from(total).unwrap_or(u32::MAX).max(MIN_WALK),
        lead: Box::new(move |conn| led(conn, library, table, id, &ids)),
    };
    let walking = table.source(conn, library, None, None)?;
    let walk = Some(Box::new(Walk::Ids(walk)));
    let walking = Source { select: id, joins: "", total: Total::Stored(total), walk, ..walking };
    request.read(conn, &walking, id_of)
}

/// The rows of `table` that `ids` names, led as one JSON array, selecting only their `id`.
fn led(
    conn: &Connection,
    library: i64,
    table: &Table,
    id: &'static [&'static str],
    ids: &[i64],
) -> rusqlite::Result<Source> {
    let source = table.source(conn, library, Some(matches_lead(ids)), None)?;
    let total = Total::Stored(i64::try_from(ids.len()).unwrap_or(i64::MAX));
    Ok(Source { select: id, joins: "", total, ..source })
}

/// The rows `ids` names, to lead a list as one JSON array, however many there are: a search's
/// matches, which the list then sorts, so it costs what they are.
pub fn matches_lead(ids: &[i64]) -> Lead {
    Lead {
        ids: "SELECT value AS id FROM json_each(?)".to_owned(),
        params: vec![Value::Text(serde_json::to_string(ids).expect("IDs are JSON"))],
        size: i64::try_from(ids.len()).unwrap_or(i64::MAX),
    }
}

/// The spec's `SearchResult`: `{"type": "track", "track": {…}}` and so on.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum SearchResult {
    Track { track: TrackSummary },
    Album { album: AlbumSummary },
    Artist { artist: ArtistSummary },
}

/// Calls `add` with each of the library's tracks, albums and artists, as the core's rule makes
/// them documents, one row at a time, so that none of them need be held at once.
pub fn documents(
    conn: &Connection,
    library: i64,
    mut add: impl FnMut(Document),
) -> rusqlite::Result<()> {
    let mut tracks = conn.prepare_cached("SELECT id, title FROM tracks WHERE library_id = ?1")?;
    let mut rows = tracks.query([library])?;
    while let Some(row) = rows.next()? {
        add(Document::from_track(row.get(0)?, row.get(1)?));
    }
    let mut albums = conn.prepare_cached("SELECT id, title FROM albums WHERE library_id = ?1")?;
    let mut rows = albums.query([library])?;
    while let Some(row) = rows.next()? {
        if let Some(album) = Document::from_album(row.get(0)?, row.get(1)?) {
            add(album);
        }
    }
    let mut artists = conn.prepare_cached("SELECT id, name FROM artists WHERE library_id = ?1")?;
    let mut rows = artists.query([library])?;
    while let Some(row) = rows.next()? {
        if let Some(artist) = Document::from_artist(row.get(0)?, row.get(1)?) {
            add(artist);
        }
    }
    Ok(())
}

/// Where a read of a library's documents in batches stands ([`documents_from`]): the table it is
/// in (tracks, albums, then artists) and the lowest ID of it not yet read.
#[derive(Debug, Default)]
pub struct Reading {
    table: usize,
    from: i64,
}

/// Calls `add` with up to `limit` of the library's [`documents`], the same ones in all, from
/// where `reading` stands, which it moves on; says whether that was all of them. Each table is
/// read in ID order, by its primary key rather than an index, as a device's file, which holds the
/// one library, is best read; the server's database holds many, so it reads [`documents`].
/// Rows changed between two calls may be missed or read twice: the caller starts again then.
pub fn documents_from(
    conn: &Connection,
    library: i64,
    reading: &mut Reading,
    limit: usize,
    mut add: impl FnMut(Document),
) -> rusqlite::Result<bool> {
    /// The tables read, in `reading.table` order, with the column each document's text comes from.
    const TABLES: [(&str, &str); 3] =
        [("tracks", "title"), ("albums", "title"), ("artists", "name")];
    let mut left = limit;
    while let Some(&(table, column)) = TABLES.get(reading.table) {
        let mut statement = conn.prepare_cached(&format!(
            "SELECT id, {column} FROM {table} NOT INDEXED \
             WHERE library_id = ?1 AND id >= ?2 ORDER BY id LIMIT ?3"
        ))?;
        let mut rows = statement.query(params![
            library,
            reading.from,
            i64::try_from(left).unwrap_or(i64::MAX)
        ])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let document = match reading.table {
                0 => Some(Document::from_track(id, row.get(1)?)),
                1 => Document::from_album(id, row.get(1)?),
                _ => Document::from_artist(id, row.get(1)?),
            };
            document.into_iter().for_each(&mut add);
            reading.from = id + 1;
            left -= 1;
        }
        if left == 0 {
            return Ok(false);
        }
        reading.table += 1;
        reading.from = 0;
    }
    Ok(true)
}

/// The search index of the library, built from its [`documents`] as they are read.
pub fn index(conn: &Connection, library: i64) -> rusqlite::Result<Index> {
    let mut builder = IndexBuilder::default();
    documents(conn, library, |document| builder.add(document))?;
    Ok(builder.finish())
}

/// How many matches a search for `q` returns in each section, from its `sectionLimit`: 5 unless
/// it names 1 to 50. [`Invalid`] for an empty `q`.
pub fn section_limit(q: &str, section_limit: Option<usize>) -> Result<usize, Invalid> {
    if q.is_empty() {
        return Err(Invalid("q must not be empty".to_owned()));
    }
    let limit = section_limit.unwrap_or(5);
    if !(1..=50).contains(&limit) {
        return Err(Invalid("sectionLimit must be from 1 to 50".to_owned()));
    }
    Ok(limit)
}

/// The kinds to search for the comma-separated section `types`. Playlists add none, since the
/// index holds no playlists and an empty section is the true answer. Lyrics are [`Invalid`] until
/// they are indexed, as a list is for a filter it does not support yet.
pub fn kinds(types: &str) -> Result<Vec<Kind>, Invalid> {
    let mut kinds = Vec::new();
    for section in types.split(',') {
        match section {
            "playlists" => {}
            "lyrics" => return Err(Invalid("lyrics cannot be searched yet".to_owned())),
            _ => {
                let Some(kind) = SEARCHED.into_iter().find(|&kind| section_type(kind) == section)
                else {
                    return Err(Invalid(
                        "types must be tracks, albums, artists, playlists, or lyrics".to_owned(),
                    ));
                };
                kinds.push(kind);
            }
        }
    }
    Ok(kinds)
}

/// For each kind in `Kind::ALL` order, the IDs of the rows of `library` of it that carry the
/// tags `filter` names, as each list's own filter links them ([`Tags::ids`]): none for a kind not
/// in `kinds`. The caller asks only for a filter that names tags; for one that names none every
/// set is empty.
pub fn tagged(
    conn: &Connection,
    library: i64,
    filter: &TagFilter,
    kinds: &[Kind],
) -> rusqlite::Result<[HashSet<i64>; Kind::ALL.len()]> {
    let mut sets: [HashSet<i64>; Kind::ALL.len()] = Default::default();
    for &kind in kinds {
        let tagged: &Tagged = match kind {
            Kind::Track => &tracks::TAGGED,
            Kind::Album => &albums::TAGGED,
            Kind::Artist => &artists::TAGGED,
            // A playlist or a tag carries no tags, so a filter keeps none of them.
            Kind::Playlist | Kind::Tag => continue,
        };
        let ids = Tags::read(conn, library, tagged, filter)?.ids(conn, tagged)?;
        sets[kind as usize] = ids.unwrap_or_default();
    }
    Ok(sets)
}

/// What `user` hid of `library`, as a search leaves it out: by [`Kind`], the tracks they hid and
/// the albums and artists every track of which they hid, with the albums and artists the tags
/// `filter` names reach only through hidden tracks; and what the hidden tracks take from the
/// counts of the albums and artists found.
pub struct Hid {
    sets: [HashSet<i64>; Kind::ALL.len()],
    losses: Losses,
}

impl Hid {
    /// Reads what `user` hid in `library`, for a search filtered by `filter`.
    pub fn read(
        conn: &Connection,
        user: i64,
        library: i64,
        filter: &TagFilter,
    ) -> rusqlite::Result<Hid> {
        let losses = hidden::losses(conn, user, library)?;
        // By `Kind`, as the tagged sets are.
        let mut sets: [HashSet<i64>; Kind::ALL.len()] = Default::default();
        sets[Kind::Track as usize] = hidden::ids(conn, user, library)?;
        sets[Kind::Album as usize] = losses.whole_albums.iter().copied().collect();
        sets[Kind::Artist as usize] = losses.whole_artists.iter().copied().collect();
        if !filter.is_empty() && !sets[Kind::Track as usize].is_empty() {
            let albums = losses.albums.iter().map(|(&album, loss)| (album, &loss.tags));
            let tagged = &albums::TAGGED;
            let lost = Tags::read(conn, library, tagged, filter)?.lost(conn, tagged, albums)?;
            sets[Kind::Album as usize].extend(lost);
            let artists = losses.artists.iter().map(|(&artist, loss)| (artist, &loss.tags));
            let tagged = &artists::TAGGED;
            let lost = Tags::read(conn, library, tagged, filter)?.lost(conn, tagged, artists)?;
            sets[Kind::Artist as usize].extend(lost);
        }
        Ok(Hid { sets, losses })
    }

    /// Whether the row `id` of `kind` is hidden, so a search leaves it out.
    pub fn contains(&self, kind: Kind, id: i64) -> bool {
        self.sets[kind as usize].contains(&id)
    }
}

/// Whether a search shows a match: what the caller hid is left out, and so, under a tag filter, is
/// what lacks the tags. `search` and `searchSection` keep by it, on the server and on a device.
pub struct Keep<'a> {
    hid: &'a Hid,
    /// By [`Kind`], what has the tags, under a tag filter.
    tagged: Option<&'a [HashSet<i64>; Kind::ALL.len()]>,
}

impl<'a> Keep<'a> {
    pub fn new(hid: &'a Hid, tagged: Option<&'a [HashSet<i64>; Kind::ALL.len()]>) -> Keep<'a> {
        Keep { hid, tagged }
    }

    /// Whether the search shows the row `id` of `kind`.
    pub fn keeps(&self, kind: Kind, id: i64) -> bool {
        !self.hid.contains(kind, id)
            && self.tagged.is_none_or(|tagged| tagged[kind as usize].contains(&id))
    }
}

/// The spec's name for the section of `kind`.
fn section_type(kind: Kind) -> &'static str {
    match kind {
        Kind::Track => "tracks",
        Kind::Album => "albums",
        Kind::Artist => "artists",
        Kind::Playlist => "playlists",
        Kind::Tag => "tags",
    }
}

/// The response to a search for `q` in `library` that found `found` for `user`, whose hidden
/// tracks `hid` says: the summaries of what the index found, as the browse lists show them to
/// `user`, and whether the library holds no track at all.
pub fn response(
    conn: &Connection,
    user: i64,
    library: i64,
    q: String,
    found: Vec<Found>,
    hid: &Hid,
) -> rusqlite::Result<SearchResponse> {
    let sections = sections(conn, user, library, found, &hid.losses)?;
    Ok(SearchResponse {
        query: q,
        top: sections.first().map(|section| section.items[0].clone()),
        sections,
        near_misses: Vec::new(),
        library_empty: library_number(conn, library, "track_count")? == 0,
    })
}

/// The summaries of what the index found, a section for each kind that has any left to show, as
/// the browse lists show them to `user`, whose hidden tracks take `losses` from them.
fn sections(
    conn: &Connection,
    user: i64,
    library: i64,
    found: Vec<Found>,
    losses: &Losses,
) -> rusqlite::Result<Vec<Section>> {
    let mut sections = Vec::with_capacity(found.len());
    for section in found {
        let ids = &section.ids;
        let items = match section.kind {
            Kind::Track => {
                let mut found =
                    by_ids(conn, tracks::SELECT, "tracks t", tracks::JOINS, library, ids, |row| {
                        tracks::list_summary(row)
                    })?;
                tracks::mark_liked(conn, user, library, &mut found)?;
                found.into_iter().map(|track| SearchResult::Track { track }).collect()
            }
            Kind::Album => by_ids(conn, albums::SELECT, "albums al", "", library, ids, |row| {
                let mut album = albums::summary(row)?;
                album.lose(losses);
                Ok(SearchResult::Album { album })
            })?,
            Kind::Artist => by_ids(conn, artists::SELECT, "artists ar", "", library, ids, |row| {
                let mut artist = artists::summary(row)?;
                artist.lose(losses);
                Ok(SearchResult::Artist { artist })
            })?,
            // Not searched yet ([`SEARCHED`]), so never found.
            Kind::Playlist | Kind::Tag => Vec::new(),
        };
        // Gone since the index was built: nothing of it is left to show.
        if !items.is_empty() {
            let kind = section_type(section.kind);
            sections.push(Section { kind, items, total: section.total });
        }
    }
    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank() -> Rank {
        let score = Score { inexact: true, typos: 2, skipped: false, prefixes: 1, extra_words: 3 };
        Rank { score, words: vec!["ca".to_owned(), "va".to_owned()], id: -4 }
    }

    #[test]
    fn a_rank_reads_back_from_its_cursor() {
        let cursor = cursor::encode(&label(Kind::Track), &rank_values(&rank()));
        assert_eq!(after(Some(&cursor), Kind::Track).unwrap(), Some(rank()));
        assert!(after(Some(&cursor), Kind::Album).is_err(), "another section's");
    }

    #[test]
    fn values_no_rank_has_are_refused() {
        for (at, value) in [(0, Value::Integer(2)), (1, Value::Integer(-1)), (5, Value::Null)] {
            let mut values = rank_values(&rank());
            values[at] = value;
            let cursor = cursor::encode(&label(Kind::Track), &values);
            assert!(after(Some(&cursor), Kind::Track).is_err(), "{values:?}");
        }
    }
}
