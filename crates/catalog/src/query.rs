//! The catalog's shared queries by name, as the API's operations are named
//! (`crates/catalog/parity/`): [`answer`] gives one's JSON from parameters spelled as the API's
//! query and path parameters. A device answers its reads with it, and the parity test asks the
//! server's database and a device's the same way. The server's handlers keep their own.

use std::fmt;

use jewelcase_core::{Index, Kind, list_hash};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::albums::{self, AlbumSort};
use crate::artists::{self, ArtistSort};
use crate::credit::ArtistCredit;
use crate::page::{Order, Request};
use crate::playlists::{self, PlaylistSort};
use crate::recipes::{self, Recipe};
use crate::tag_filter::{TagFilter, Tags};
use crate::tags::{self, TagSort};
use crate::tracks::{self, TrackOrder, TrackSort};
use crate::{AnyId, Id, Invalid, search};

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

/// The number of the canonical ID that `params` holds under `key`, as a `get*` operation's path
/// parameter; an error if it is missing or not canonical.
fn path_id(params: &Value, key: &str) -> Result<i64, Error> {
    params[key]
        .as_str()
        .and_then(Id::canonical)
        .map(|Id(id)| id)
        .ok_or_else(|| Error(format!("invalid params: {key} is not an ID")))
}

/// A canonical ID's number, or an error naming `what`.
pub fn number(id: &str, what: &str) -> Result<i64, Error> {
    Id::canonical(id).map(|Id(id)| id).ok_or_else(|| Error(format!("{what} is not an ID")))
}

/// `value` as JSON, for the answer of a query.
fn json<T: Serialize>(value: T) -> Result<Value, Error> {
    Ok(serde_json::to_value(value)?)
}

/// The search index, or the error a search gives without one.
fn built(index: Option<&Index>) -> Result<&Index, Error> {
    index.ok_or_else(|| Error("the search index is not built".to_owned()))
}

/// Answers the operation `name` (an API operation's ID, such as `listTracks`) with `params`, a
/// JSON object spelled as the API's query and path parameters, for `user` in `library`, with the
/// JSON the server sends. `index` is the library's search index, which `search` and search
/// recipes need. An error for an unknown name, parameters the API would refuse, a missing index
/// where one is needed, or a query that fails.
pub fn answer(
    conn: &Connection,
    user: i64,
    library: i64,
    name: &str,
    params: Value,
    index: Option<&Index>,
) -> Result<Value, Error> {
    match name {
        "listTracks" => {
            let p: TrackParams = serde_json::from_value(params)?;
            let (sort, order) =
                TrackOrder { sort: p.sort, order: p.order }.keyset(p.album_id.is_some());
            let request = Request::new(sort, order, p.cursor, p.limit)?;
            let tags = TagFilter::new(p.tag_all.as_deref(), p.tag_any.as_deref())?;
            let credit = p.artist_credit.unwrap_or_default();
            let page =
                tracks::list(conn, library, request, p.album_id, p.artist_id, credit, &tags)?;
            json(page)
        }
        "listTracksByIds" => {
            let p: IdsParams = serde_json::from_value(params)?;
            let ids: Vec<i64> = p.ids.split(',').filter_map(|id| AnyId::from(id).0).collect();
            json(tracks::by_ids(conn, library, &ids)?)
        }
        "getTrack" => json(tracks::track(conn, library, path_id(&params, "trackId")?)?),
        "listAlbums" => {
            let p: AlbumParams = serde_json::from_value(params)?;
            let request = Request::new(p.sort.sort(), p.order, p.cursor, p.limit)?;
            let tags = TagFilter::new(p.tag_all.as_deref(), p.tag_any.as_deref())?;
            let credit = p.artist_credit.unwrap_or_default();
            json(albums::list(conn, library, request, p.artist_id, credit, &tags)?)
        }
        "getAlbum" => json(albums::album(conn, library, path_id(&params, "albumId")?)?),
        "listArtists" => {
            let p: ArtistParams = serde_json::from_value(params)?;
            let request = Request::new(p.sort.sort(), p.order, p.cursor, p.limit)?;
            let tags = TagFilter::new(p.tag_all.as_deref(), p.tag_any.as_deref())?;
            json(artists::list(conn, library, request, &tags)?)
        }
        "getArtist" => json(artists::artist(conn, library, path_id(&params, "artistId")?)?),
        "listTagTypes" => json(tags::types(conn, library)?),
        "listTags" => {
            let p: TagParams = serde_json::from_value(params)?;
            let request = Request::new(p.sort.sort(), p.order, p.cursor, p.limit)?;
            json(tags::list(conn, library, request, p.r#type, p.q.as_deref())?)
        }
        "getTag" => json(tags::find(conn, library, path_id(&params, "tagId")?)?),
        "listPlaylists" => {
            let p: PlaylistParams = serde_json::from_value(params)?;
            let request = Request::new(p.sort.sort(), p.order, p.cursor, p.limit)?;
            json(playlists::list(conn, user, library, request, p.q.as_deref())?)
        }
        "getPlaylist" => {
            json(playlists::get(conn, user, library, path_id(&params, "playlistId")?)?)
        }
        "listPlaylistItems" => {
            let p: ItemParams = serde_json::from_value(params)?;
            let id = number(&p.playlist_id, "playlistId")?;
            json(playlists::items(conn, user, library, id, p.cursor.as_deref(), p.limit)?)
        }
        "search" => {
            let p: SearchParams = serde_json::from_value(params)?;
            let kinds = p.types.as_deref().map_or_else(|| Ok(Kind::ALL.to_vec()), search::kinds)?;
            let filter = TagFilter::new(p.tag_all.as_deref(), p.tag_any.as_deref())?;
            let tagged = if filter.is_empty() {
                None
            } else {
                Some(search::tagged(conn, library, &filter, &kinds)?)
            };
            let found = built(index)?.search_within(
                &p.q,
                &kinds,
                p.section_limit.unwrap_or(5),
                |kind, id| tagged.as_ref().is_none_or(|sets| sets[kind as usize].contains(&id)),
            );
            json(search::response(conn, library, p.q, found)?)
        }
        "resolveRecipe" => {
            let RecipeParams { recipe } = serde_json::from_value(params)?;
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
) -> Result<Value, Error> {
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
    Ok(match resolved {
        Some((ids, stored)) => json!({
            "ids": ids.iter().map(u64::to_string).collect::<Vec<_>>(),
            "hash": list_hash(&ids),
            "recipe": stored,
        }),
        None => Value::Null,
    })
}
