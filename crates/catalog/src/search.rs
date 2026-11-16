//! Search: the documents a library's search index is built from, and the responses a search
//! gives (`search` and `searchSection`, `api/openapi.yaml`). The index itself is the core's
//! (`jewelcase_core::Index`), which the server keeps in memory and a device builds from the same
//! documents.

use std::collections::HashSet;

use jewelcase_core::{
    Document, Index, IndexBuilder, Kind, Rank, Section as Found, SectionPage as FoundPage,
};
use rusqlite::types::Value;
use rusqlite::{Connection, params};
use serde::de::IntoDeserializer;
use serde::de::value::StrDeserializer;
use serde::{Deserialize, Serialize};

use crate::Invalid;
use crate::albums::{self, AlbumSort, AlbumSummary};
use crate::artists::{self, ArtistSort, ArtistSummary};
use crate::by_ids::by_ids;
use crate::cursor;
use crate::hidden::{self, Losses};
use crate::page::{self, Lead, Order, Page, Request, Sort, library_number};
use crate::tag_filter::{TagFilter, Tagged, Tags};
use crate::tracks::{self, TrackSort, TrackSummary};

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
            name => Kind::ALL
                .into_iter()
                .find(|&kind| section_type(kind) == name)
                .map(SectionType::Kind),
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
    /// How the section `section` of `kind` is read for a `sort` and `order` (the list's, or the
    /// ranking without them), continuing after `cursor`, `limit` matches at a time. A sort the
    /// section's list does not take, an `order` with no sort, and a cursor of another section or
    /// sort, are [`Invalid`].
    pub fn new(
        section: &str,
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
                Ok(SectionReading::Ranked { after: after(cursor.as_deref(), section)?, limit })
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

/// The rank a page of the section `section` continues after, from its `cursor`: a cursor of
/// another section, or none of ours, is [`Invalid`].
fn after(cursor: Option<&str>, section: &str) -> Result<Option<Rank>, Invalid> {
    let Some(cursor) = cursor else { return Ok(None) };
    match cursor::decode(cursor, &label(section), 1)?.as_slice() {
        [Value::Blob(bytes)] => Rank::from_bytes(bytes),
        _ => None,
    }
    .map(Some)
    .ok_or_else(|| Invalid("cursor is not valid for this list".to_owned()))
}

/// The label of a section's cursors, so one is only accepted where it came from.
fn label(section: &str) -> String {
    format!("search.{section}")
}

/// The keyset of the list sort `name` of the section of `kind`: one of `listTracks`',
/// `listAlbums`', or `listArtists`'. Any other is [`Invalid`].
fn list_sort(kind: Kind, name: &str) -> Result<&'static Sort, Invalid> {
    let name: StrDeserializer<serde::de::value::Error> = name.into_deserializer();
    match kind {
        Kind::Track => TrackSort::deserialize(name).map(TrackSort::sort),
        Kind::Album => AlbumSort::deserialize(name).map(AlbumSort::sort),
        Kind::Artist => ArtistSort::deserialize(name).map(ArtistSort::sort),
    }
    .map_err(|_| Invalid(format!("sort must be one the {} list takes", section_type(kind))))
}

impl SectionPage {
    /// The page of a section that has nothing in it yet, for a search for `q`.
    pub fn empty(q: String) -> SectionPage {
        SectionPage { query: q, items: Vec::new(), next_cursor: None, total: 0 }
    }
}

/// A page of the section of `kind` of a search for `q` in `library` with `index`, of what `keep`
/// keeps, read as `reading` says, as the browse lists show it to `user`, whose hidden tracks
/// `hid` says, with the cursor to the next.
#[expect(
    clippy::too_many_arguments,
    reason = "a search's caller, query, index and what it keeps, as the server and a device each hold them"
)]
pub fn section(
    conn: &Connection,
    user: i64,
    library: i64,
    q: String,
    kind: Kind,
    reading: SectionReading,
    index: &Index,
    keep: impl Fn(Kind, i64) -> bool,
    hid: &Hid,
) -> rusqlite::Result<SectionPage> {
    match reading {
        SectionReading::Ranked { after, limit } => {
            let found = index.page(&q, kind, after.as_ref(), limit as usize, keep);
            let label = label(section_type(kind));
            let next_cursor =
                found.next.map(|rank| cursor::encode(&label, &[Value::Blob(rank.to_bytes())]));
            let FoundPage { ids, total, .. } = found;
            let items =
                sections(conn, user, library, vec![Found { kind, ids, total }], &hid.losses)?
                    .pop()
                    .map_or_else(Vec::new, |section| section.items);
            Ok(SectionPage { query: q, items, next_cursor, total })
        }
        SectionReading::Sorted(request) => {
            // Every match: the sort orders them, whatever order they come in.
            let ids = index.page(&q, kind, None, usize::MAX, keep).ids;
            let page = sorted(conn, user, library, kind, request, &ids, &hid.losses)?;
            let total = usize::try_from(page.total).unwrap_or(0);
            Ok(SectionPage { query: q, items: page.items, next_cursor: page.next_cursor, total })
        }
    }
}

/// The page `request` asks for of the matches `ids` of `kind`, sorted as their list sorts
/// them, as `user` sees them, whose hidden tracks take `losses` from albums and artists. The
/// matches are already only what the search keeps, and they lead the page however many there
/// are, as one JSON array, so the cost is theirs and not the library's.
fn sorted(
    conn: &Connection,
    user: i64,
    library: i64,
    kind: Kind,
    request: Request,
    ids: &[i64],
    losses: &Losses,
) -> rusqlite::Result<Page<SearchResult>> {
    let lead = Lead {
        ids: "SELECT value AS id FROM json_each(?)".to_owned(),
        params: vec![Value::Text(serde_json::to_string(ids).expect("IDs are JSON"))],
        size: i64::try_from(ids.len()).unwrap_or(i64::MAX),
    };
    Ok(match kind {
        Kind::Track => {
            let source = tracks::TRACKS.source(conn, library, Some(lead), None)?;
            let mut page = request.read(conn, &source, tracks::list_summary)?;
            tracks::mark_liked(conn, user, library, &mut page.items)?;
            let items = page.items.into_iter().map(|track| SearchResult::Track { track });
            Page { items: items.collect(), next_cursor: page.next_cursor, total: page.total }
        }
        Kind::Album => {
            let source = albums::ALBUMS.source(conn, library, Some(lead), None)?;
            request.read(conn, &source, |row| {
                let mut album = albums::summary(row)?;
                album.lose(losses);
                Ok(SearchResult::Album { album })
            })?
        }
        Kind::Artist => {
            let source = artists::ARTISTS.source(conn, library, Some(lead), None)?;
            request.read(conn, &source, |row| {
                let mut artist = artists::summary(row)?;
                artist.lose(losses);
                Ok(SearchResult::Artist { artist })
            })?
        }
    })
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
                let Some(kind) = Kind::ALL.into_iter().find(|&kind| section_type(kind) == section)
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
) -> rusqlite::Result<[HashSet<i64>; 3]> {
    let mut sets: [HashSet<i64>; 3] = Default::default();
    for &kind in kinds {
        let tagged: &Tagged = match kind {
            Kind::Track => &tracks::TAGGED,
            Kind::Album => &albums::TAGGED,
            Kind::Artist => &artists::TAGGED,
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
    sets: [HashSet<i64>; 3],
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
        let mut sets: [HashSet<i64>; 3] = [
            hidden::ids(conn, user, library)?,
            losses.whole_albums.iter().copied().collect(),
            losses.whole_artists.iter().copied().collect(),
        ];
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

/// The spec's name for the section of `kind`.
fn section_type(kind: Kind) -> &'static str {
    match kind {
        Kind::Track => "tracks",
        Kind::Album => "albums",
        Kind::Artist => "artists",
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
        };
        // Gone since the index was built: nothing of it is left to show.
        if !items.is_empty() {
            let kind = section_type(section.kind);
            sections.push(Section { kind, items, total: section.total });
        }
    }
    Ok(sections)
}
