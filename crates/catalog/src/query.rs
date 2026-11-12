//! The catalog's shared queries by name, as the API's operations are named
//! (`crates/catalog/parity/`): [`answer`] gives one's JSON from parameters spelled as the API's
//! query and path parameters. A device answers its reads with it, and the parity test asks the
//! server's database and a device's the same way. The server's handlers keep their own.

use std::collections::HashMap;
use std::fmt::{self, Write};

use jewelcase_core::{Index, Kind, list_hash};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::albums::{self, AlbumSort};
use crate::artists::{self, ArtistSort};
use crate::credit::ArtistCredit;
use crate::page::{Order, Request, Totals};
use crate::playlists::{self, PlaylistSort};
use crate::recipes::{self, Recipe};
use crate::tag_filter::{TagFilter, Tags};
use crate::tags::{self, TagSort};
use crate::tracks::{self, TrackOrder, TrackSort};
use crate::{AnyId, Id, Invalid, hidden, recent_searches, search};

/// Why a query could not be answered; the message says why, safe to show a client.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    /// Writes the message alone.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<Invalid> for Error {
    /// Keeps the API's own reason for refusing the parameters.
    fn from(Invalid(why): Invalid) -> Error {
        Error(why)
    }
}

impl From<rusqlite::Error> for Error {
    /// Reports a failed query by SQLite's message.
    fn from(error: rusqlite::Error) -> Error {
        Error(error.to_string())
    }
}

impl From<serde_json::Error> for Error {
    /// Reports parameters that do not parse as a request of the operation.
    fn from(error: serde_json::Error) -> Error {
        Error(format!("invalid params: {error}"))
    }
}

impl From<playlists::ItemsError> for Error {
    /// Reports a refused playlist cursor, a missing playlist or an incomplete page by the error's
    /// own message.
    fn from(error: playlists::ItemsError) -> Error {
        Error(error.to_string())
    }
}

/// A `listTracks` request's parameters, as the API spells them in a query. Like the server's
/// handlers, each takes the API's default for what it leaves out.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrackParams {
    #[serde(default)]
    sort: TrackSort,
    #[serde(default)]
    order: Order,
    album_id: Option<AnyId>,
    artist_id: Option<AnyId>,
    artist_credit: Option<ArtistCredit>,
    tag_all: Option<String>,
    tag_any: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// A `listAlbums` request's parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AlbumParams {
    #[serde(default)]
    sort: AlbumSort,
    #[serde(default)]
    order: Order,
    artist_id: Option<AnyId>,
    artist_credit: Option<ArtistCredit>,
    tag_all: Option<String>,
    tag_any: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// A `listArtists` request's parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArtistParams {
    #[serde(default)]
    sort: ArtistSort,
    #[serde(default)]
    order: Order,
    tag_all: Option<String>,
    tag_any: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// A `listTags` request's parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TagParams {
    #[serde(default)]
    sort: TagSort,
    #[serde(default)]
    order: Order,
    r#type: Option<String>,
    q: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// A `listPlaylists` request's parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlaylistParams {
    #[serde(default)]
    sort: PlaylistSort,
    #[serde(default)]
    order: Order,
    q: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// A `listPlaylistItems` request's parameters, with its path's playlist.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ItemParams {
    playlist_id: String,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// A `listHiddenTracks` request's parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HiddenParams {
    order: Option<Order>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// A `listRecentSearches` request's parameters: none.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParams {}

/// A `search` request's parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchParams {
    q: String,
    types: Option<String>,
    tag_all: Option<String>,
    tag_any: Option<String>,
    section_limit: Option<usize>,
}

/// A `listTracksByIds` request's parameters.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdsParams {
    ids: String,
}

/// A recipe's parameters.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeParams {
    recipe: Recipe,
}

/// The number of the canonical ID that `params` (JSON text of an object) holds under `key`, as a
/// `get*` operation's path parameter; an error if it is missing or not canonical.
fn path_id(params: &str, key: &str) -> Result<i64, Error> {
    let mut fields: HashMap<String, String> = serde_json::from_str(params)?;
    fields
        .remove(key)
        .as_deref()
        .and_then(Id::canonical)
        .map(|Id(id)| id)
        .ok_or_else(|| Error(format!("invalid params: {key} is not an ID")))
}

/// A canonical ID's number, or an error naming `what`.
pub fn number(id: &str, what: &str) -> Result<i64, Error> {
    Id::canonical(id).map(|Id(id)| id).ok_or_else(|| Error(format!("{what} is not an ID")))
}

/// `value` as the JSON text of a query's answer, written once.
fn json<T: Serialize>(value: T) -> Result<String, Error> {
    Ok(serde_json::to_string(&value)?)
}

/// The search index, or the error a search gives without one.
fn built(index: Option<&Index>) -> Result<&Index, Error> {
    index.ok_or_else(|| Error("the search index is not built".to_owned()))
}

/// Answers the operation `name` (an API operation's ID, such as `listTracks`) with `params`, the
/// text of a JSON object spelled as the API's query and path parameters, for `user` in `library`,
/// with the JSON text the server sends. The answer is serialized once, straight to the string.
/// `index` is the library's search index, which `search` and search recipes need. `totals`, if
/// given, remembers the counted totals of lists, and must be cleared before any write to `conn`'s
/// database is seen. An error for an unknown name, parameters the API would refuse, a missing
/// index where one is needed, or a query that fails.
pub fn answer(
    conn: &Connection,
    user: i64,
    library: i64,
    name: &str,
    params: &str,
    index: Option<&Index>,
    totals: Option<&Totals>,
) -> Result<String, Error> {
    let memo = totals.map(|totals| totals.of(user));
    match name {
        "listTracks" => {
            let p: TrackParams = serde_json::from_str(params)?;
            let (sort, order) =
                TrackOrder { sort: p.sort, order: p.order }.keyset(p.album_id.is_some());
            let request = Request::new(sort, order, p.cursor, p.limit)?.memoized(memo);
            let tags = TagFilter::new(p.tag_all.as_deref(), p.tag_any.as_deref())?;
            let credit = p.artist_credit.unwrap_or_default();
            let page =
                tracks::list(conn, user, library, request, p.album_id, p.artist_id, credit, &tags)?;
            json(page)
        }
        "listTracksByIds" => {
            let p: IdsParams = serde_json::from_str(params)?;
            let ids: Vec<i64> = p.ids.split(',').filter_map(|id| AnyId::from(id).0).collect();
            json(tracks::by_ids(conn, user, library, &ids)?)
        }
        "getTrack" => json(tracks::track(conn, user, library, path_id(params, "trackId")?)?),
        "listAlbums" => {
            let p: AlbumParams = serde_json::from_str(params)?;
            let request = Request::new(p.sort.sort(), p.order, p.cursor, p.limit)?.memoized(memo);
            let tags = TagFilter::new(p.tag_all.as_deref(), p.tag_any.as_deref())?;
            let credit = p.artist_credit.unwrap_or_default();
            json(albums::list(conn, user, library, request, p.artist_id, credit, &tags)?)
        }
        "getAlbum" => json(albums::album(conn, user, library, path_id(params, "albumId")?)?),
        "listArtists" => {
            let p: ArtistParams = serde_json::from_str(params)?;
            let request = Request::new(p.sort.sort(), p.order, p.cursor, p.limit)?.memoized(memo);
            let tags = TagFilter::new(p.tag_all.as_deref(), p.tag_any.as_deref())?;
            json(artists::list(conn, user, library, request, &tags)?)
        }
        "getArtist" => json(artists::artist(conn, user, library, path_id(params, "artistId")?)?),
        "listTagTypes" => json(tags::types(conn, user, library)?),
        "listTags" => {
            let p: TagParams = serde_json::from_str(params)?;
            let request = Request::new(p.sort.sort(), p.order, p.cursor, p.limit)?.memoized(memo);
            json(tags::list(conn, user, library, request, p.r#type, p.q.as_deref())?)
        }
        "getTag" => json(tags::find(conn, user, library, path_id(params, "tagId")?)?),
        "listPlaylists" => {
            let p: PlaylistParams = serde_json::from_str(params)?;
            let request = Request::new(p.sort.sort(), p.order, p.cursor, p.limit)?.memoized(memo);
            json(playlists::list(conn, user, library, request, p.q.as_deref())?)
        }
        "getPlaylist" => json(playlists::get(conn, user, library, path_id(params, "playlistId")?)?),
        "listPlaylistItems" => {
            let p: ItemParams = serde_json::from_str(params)?;
            let id = number(&p.playlist_id, "playlistId")?;
            json(playlists::items(conn, user, library, id, p.cursor.as_deref(), p.limit)?)
        }
        "search" => {
            let p: SearchParams = serde_json::from_str(params)?;
            let limit = search::section_limit(&p.q, p.section_limit)?;
            let kinds = p.types.as_deref().map_or_else(|| Ok(Kind::ALL.to_vec()), search::kinds)?;
            let filter = TagFilter::new(p.tag_all.as_deref(), p.tag_any.as_deref())?;
            let tagged = if filter.is_empty() {
                None
            } else {
                Some(search::tagged(conn, library, &filter, &kinds)?)
            };
            let hid = search::Hid::read(conn, user, library, &filter)?;
            let found = built(index)?.search_within(&p.q, &kinds, limit, |kind, id| {
                !hid.contains(kind, id)
                    && tagged.as_ref().is_none_or(|sets| sets[kind as usize].contains(&id))
            });
            json(search::response(conn, user, library, p.q, found, &hid)?)
        }
        "listHiddenTracks" => {
            let p: HiddenParams = serde_json::from_str(params)?;
            let order = p.order.unwrap_or(hidden::DEFAULT_ORDER);
            let request =
                Request::new(&hidden::HIDDEN_AT, order, p.cursor, p.limit)?.memoized(memo);
            json(hidden::list(conn, user, library, request)?)
        }
        "listRecentSearches" => {
            let NoParams {} = serde_json::from_str(params)?;
            // The window the server keeps them for is left to the feed's deletions.
            json(recent_searches::list(conn, user, library, i64::MIN)?)
        }
        "resolveRecipe" => {
            let RecipeParams { recipe } = serde_json::from_str(params)?;
            resolve(conn, user, library, &recipe, index)
        }
        _ => Err(Error(format!("unknown query {name}"))),
    }
}

/// The tracks `recipe` resolves to with their list hash and stored form, or null if it names a
/// playlist that is not the user's. A search recipe ranks with `index`.
fn resolve(
    conn: &Connection,
    user: i64,
    library: i64,
    recipe: &Recipe,
    index: Option<&Index>,
) -> Result<String, Error> {
    let tags = recipe.check()?;
    let mut failed = None;
    let ranked = |q: &str| {
        let found = (|| {
            let index = built(index)?;
            let tagged =
                Tags::read(conn, library, &tracks::TAGGED, &tags)?.ids(conn, &tracks::TAGGED)?;
            Ok::<_, Error>(index.search_within(q, &[Kind::Track], usize::MAX, |_, id| {
                tagged.as_ref().is_none_or(|tagged| tagged.contains(&id))
            }))
        })();
        match found {
            Ok(found) => found.into_iter().next().map(|section| section.ids).unwrap_or_default(),
            Err(error) => {
                failed = Some(error);
                Vec::new()
            }
        }
    };
    let resolved = recipes::resolve(conn, user, library, recipe, &tags, ranked)?;
    if let Some(error) = failed {
        return Err(error);
    }
    let Some((ids, stored)) = resolved else { return Ok("null".to_owned()) };
    // About 20 digits and punctuation an ID, written in place rather than built as a tree.
    let mut out = String::with_capacity(ids.len() * 22 + 256);
    out.push_str("{\"ids\":[");
    for (n, id) in ids.iter().enumerate() {
        let comma = if n == 0 { "" } else { "," };
        write!(out, "{comma}\"{id}\"").expect("a string takes writes");
    }
    write!(out, "],\"hash\":\"{}\",\"recipe\":{}}}", list_hash(&ids), json(stored)?)
        .expect("a string takes writes");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::create_schema;

    /// A device database with a library of three tracks.
    fn device() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        create_schema(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO libraries (id, track_count) VALUES (1, 3); \
             INSERT INTO albums (id, library_id, title, sort_key, artist_sort_key, duration_us, \
                 added_at) VALUES (1, 1, 'Album', x'', x'', 1, 1); \
             INSERT INTO tracks (id, library_id, album_id, file_size, title, sort_key, artist_sort_key, \
                 album_sort_key, codec, container, lossless, sample_rate_hz, channels, \
                 duration_us, added_at) \
             SELECT i, 1, 1, 1, 'Track ' || i, CAST(i AS BLOB), x'', x'', 'flac', 'flac', 1, 44100, \
                 2, 1000000, i FROM (SELECT 1 AS i UNION SELECT 2 UNION SELECT 3);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn a_page_is_written_in_the_order_of_its_structs_not_of_a_tree_with_sorted_keys() {
        let conn = device();
        let text = answer(&conn, 1, 1, "listTracks", "{}", None, None).unwrap();
        let tree: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(tree["items"].as_array().unwrap().len(), 3);
        assert_ne!(text, tree.to_string(), "no tree stood between the rows and the text");
    }

    #[test]
    fn a_recipe_is_written_as_its_ids_hash_and_stored_form() {
        let conn = device();
        let recipe = r#"{"recipe":{"type":"library","sort":"name","order":"asc","filters":{}}}"#;
        let text = answer(&conn, 1, 1, "resolveRecipe", recipe, None, None).unwrap();
        let answer: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(answer["ids"].as_array().unwrap().len(), 3);
        assert_eq!(answer["hash"].as_str().unwrap().len(), 64);
        assert_eq!(answer["recipe"]["type"], "library");
        assert!(text.starts_with(r#"{"ids":["#));
    }

    /// A device refuses the search parameters the server refuses, before it needs its index.
    #[test]
    fn a_search_refuses_an_empty_q_and_a_section_limit_past_50() {
        let conn = device();
        for params in
            [r#"{"q":""}"#, r#"{"q":"a","sectionLimit":500}"#, r#"{"q":"a","sectionLimit":0}"#]
        {
            let refused = answer(&conn, 1, 1, "search", params, None, None).unwrap_err();
            assert!(
                refused.0.starts_with("q must") || refused.0.starts_with("sectionLimit"),
                "{refused}"
            );
        }
    }
}
