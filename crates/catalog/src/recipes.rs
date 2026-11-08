//! Recipes: how a list is made, and resolving one into its track IDs (`design/queue.md` §2, §3).
//!
//! A start, and later an add, names a recipe instead of sending tracks. Resolving it runs the
//! recipe against the library to the same rows, in the same order, as a view of it pages in to
//! its end, a missing track in place. The core keeps a recipe's fields as
//! text ([`jewelcase_core::Recipe`]); here they are typed as the API needs them. A device
//! resolves with the same code, so the core's list hash of the IDs matches the server's.

use std::fmt;

use jewelcase_core::{Recipe as Stored, SortField, SortRow, sort_rows};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::by_ids::by_ids;
use crate::credit::ArtistCredit;
use crate::page::Order;
use crate::playlists;
use crate::tag_filter::{TagFilter, Tags};
use crate::time::rfc3339;
use crate::tracks::{self, TrackOrder, TrackSort};
use crate::{AnyId, Invalid};

/// How a source's list is made, as a request sends it: the spec's `QueueRecipe` without
/// `tracks`, which is a list the device sent.
#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Recipe {
    /// An album's tracks under a filter, in a sort.
    Album {
        album_id: String,
        #[serde(default)]
        filters: Map<String, Value>,
        sort: TrackSort,
        order: Order,
    },
    /// An artist's tracks, credited as `artist_credit` says, under a filter, in a sort.
    Artist {
        artist_id: String,
        scope: Scope,
        artist_credit: ArtistCredit,
        #[serde(default)]
        filters: Map<String, Value>,
        sort: TrackSort,
        order: Order,
    },
    /// A library's tracks under a browse filter, in a sort.
    Library { filters: Map<String, Value>, sort: TrackSort, order: Order },
    /// The tracks a search finds under a filter, in its ranking.
    Search {
        q: String,
        #[serde(default)]
        filters: Map<String, Value>,
    },
    /// The caller's own playlist at its current version, under a filter, in the sort its page
    /// offers or, with none, in its own order.
    Playlist {
        playlist_id: String,
        #[serde(default)]
        filters: Map<String, Value>,
        sort: Option<SortField>,
        order: Option<Order>,
    },
}

/// Which of an artist's tracks a recipe covers. Only `all` is supported so far, since the other
/// scopes of an artist's page are albums, or a personal ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    All,
}

/// The spec's `BrowseFilter` as far as a recipe supports it: the tags, as a view's `tagAll` and
/// `tagAny` name them. Any other filter is refused.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct Filters {
    tag_all: Vec<String>,
    tag_any: Vec<String>,
}

/// The tag filter `filters` names, as a list reads it: [`Invalid`] when it holds any other
/// filter, or more than `MAX_TAGS` tags.
fn tag_filter(filters: &Map<String, Value>) -> Result<TagFilter, Invalid> {
    let Filters { tag_all, tag_any } = serde_json::from_value(Value::Object(filters.clone()))
        .map_err(|e| Invalid(format!("A recipe's filters can only be tags: {e}")))?;
    let join = |ids: Vec<String>| (!ids.is_empty()).then(|| ids.join(","));
    TagFilter::new(join(tag_all).as_deref(), join(tag_any).as_deref())
}

impl Recipe {
    /// The recipe as the core keeps it, which is how a snapshot shows it. A playlist's has no
    /// version until [`resolve`] gives it one.
    pub fn stored(&self) -> Stored {
        match self {
            Recipe::Album { album_id, filters, sort, order } => {
                let (sort, order) = names(*sort, *order);
                Stored::Album { album_id: album_id.clone(), filters: filters.clone(), sort, order }
            }
            Recipe::Artist { artist_id, artist_credit, filters, sort, order, .. } => {
                let (sort, order) = names(*sort, *order);
                Stored::Artist {
                    artist_id: artist_id.clone(),
                    scope: "all".to_owned(),
                    artist_credit: artist_credit.name().to_owned(),
                    filters: filters.clone(),
                    sort,
                    order,
                }
            }
            Recipe::Library { filters, sort, order } => {
                let (sort, order) = names(*sort, *order);
                Stored::Library { filters: filters.clone(), sort, order }
            }
            Recipe::Search { q, filters } => {
                Stored::Search { q: q.clone(), filters: filters.clone() }
            }
            Recipe::Playlist { playlist_id, filters, sort, order } => Stored::Playlist {
                playlist_id: playlist_id.clone(),
                version: None,
                filters: filters.clone(),
                sort: sort.map(|sort| sort.name().to_owned()),
                order: order
                    .map(|order| (if order == Order::Asc { "asc" } else { "desc" }).to_owned()),
            },
        }
    }

    /// Refuses what cannot be resolved ([`Invalid`]), and gives the tag filter.
    pub fn check(&self) -> Result<TagFilter, Invalid> {
        match self {
            Recipe::Search { q, .. } if q.is_empty() => {
                Err(Invalid("q must not be empty".to_owned()))
            }
            Recipe::Playlist { sort, order, .. } if sort.is_some() != order.is_some() => {
                Err(Invalid("A playlist recipe's sort and order come together.".to_owned()))
            }
            Recipe::Album { filters, .. }
            | Recipe::Playlist { filters, .. }
            | Recipe::Artist { filters, .. }
            | Recipe::Library { filters, .. }
            | Recipe::Search { filters, .. } => tag_filter(filters),
        }
    }
}

/// Why a recipe could not be resolved.
#[derive(Debug)]
pub enum ResolveError {
    /// A track a filtered or sorted playlist recipe names is not in the catalog, so its tags and
    /// sort keys are unknown and the list would come up short. Never on the server, where
    /// retention keeps every track an item names; on a device, a track the library feed has not
    /// brought yet.
    Incomplete,
    Sql(rusqlite::Error),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::Incomplete => f.write_str("a playlist item's track is not here yet"),
            ResolveError::Sql(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ResolveError {}

impl From<rusqlite::Error> for ResolveError {
    fn from(error: rusqlite::Error) -> ResolveError {
        ResolveError::Sql(error)
    }
}

/// `sort` and `order` as a stored recipe spells them.
fn names(sort: TrackSort, order: Order) -> (String, String) {
    let (sort, order) = TrackOrder { sort, order }.names();
    (sort.to_owned(), order.to_owned())
}

/// The track IDs `recipe` resolves to in `library`, in order, with no paging, and the recipe as
/// stored, which for a playlist records the version read. `tags` is what [`Recipe::check`] gave.
/// An album, artist or tag the library lacks resolves to no tracks, and a missing track stays in
/// place, as in a view. A playlist recipe resolves only `user`'s own playlist in the library, as
/// the playlist endpoints find it: `None` if it is not theirs or does not exist, and
/// [`ResolveError::Incomplete`] if it is filtered or sorted and an item's track row is absent.
/// `ranked` is called only for a search recipe, with its `q`, and gives
/// the IDs of the library's tracks that match it, best first and only those that carry the tags,
/// from the search index, which the catalog does not hold. Like every query here, this does not
/// check that the caller reaches the library.
pub fn resolve(
    conn: &Connection,
    user: i64,
    library: i64,
    recipe: &Recipe,
    tags: &TagFilter,
    ranked: impl FnOnce(&str) -> Vec<i64>,
) -> Result<Option<(Vec<u64>, Stored)>, ResolveError> {
    let mut stored = recipe.stored();
    let ids = match recipe {
        Recipe::Album { album_id, sort, order, .. } => {
            let album = Some(AnyId::from(album_id.as_str()));
            let sort = TrackOrder { sort: *sort, order: *order };
            tracks::resolve(conn, library, album, None, ArtistCredit::Any, tags, sort)?
        }
        Recipe::Artist { artist_id, artist_credit, sort, order, scope: Scope::All, .. } => {
            let artist = Some(AnyId::from(artist_id.as_str()));
            let sort = TrackOrder { sort: *sort, order: *order };
            tracks::resolve(conn, library, None, artist, *artist_credit, tags, sort)?
        }
        Recipe::Library { sort, order, .. } => {
            let sort = TrackOrder { sort: *sort, order: *order };
            tracks::resolve(conn, library, None, None, ArtistCredit::Any, tags, sort)?
        }
        Recipe::Search { q, .. } => in_library(conn, library, &ranked(q))?,
        Recipe::Playlist { playlist_id, sort, order, .. } => {
            let Some(playlist) = AnyId::from(playlist_id.as_str()).0 else { return Ok(None) };
            let sort = sort.map(|sort| (sort, *order == Some(Order::Desc)));
            let Some((ids, version)) = playlist_ids(conn, user, library, playlist, sort, tags)?
            else {
                return Ok(None);
            };
            if let Stored::Playlist { version: read, .. } = &mut stored {
                *read = Some(version);
            }
            ids
        }
    };
    Ok(Some((ids, stored)))
}

/// The IDs of the items of `user`'s playlist `playlist` in `library`, and the playlist's
/// version, from one read: its items in key order, a track as often as it holds it and a missing
/// track in place, kept if they carry the tags `tags` names, then in `sort`'s order if given
/// (`SortField` and whether descending) by the core's comparator over the keys the playlist's
/// page sorts by. `None` for a playlist that is not the user's own in the library;
/// [`ResolveError::Incomplete`] when it is filtered or sorted and an item's track row is absent.
fn playlist_ids(
    conn: &Connection,
    user: i64,
    library: i64,
    playlist: i64,
    sort: Option<(SortField, bool)>,
    tags: &TagFilter,
) -> Result<Option<(Vec<u64>, i64)>, ResolveError> {
    let Some((version, mut items)) = playlists::all_items(conn, user, library, playlist)? else {
        return Ok(None);
    };
    let tagged = Tags::read(conn, library, &tracks::TAGGED, tags)?.ids(conn, &tracks::TAGGED)?;
    if tagged.is_none() && sort.is_none() {
        let ids = items.iter().map(|(track, _)| track.cast_unsigned()).collect();
        return Ok(Some((ids, version)));
    }
    let mut wanted: Vec<i64> = items.iter().map(|(track, _)| *track).collect();
    wanted.sort_unstable();
    wanted.dedup();
    let keys = tracks::sort_keys(conn, library, &wanted)?;
    // A track without a row has unknown tags and keys; dropping it would shorten the list.
    if keys.len() < wanted.len() {
        return Err(ResolveError::Incomplete);
    }
    if let Some(tagged) = tagged {
        items.retain(|(track, _)| tagged.contains(track));
    }
    let Some((field, descending)) = sort else {
        let ids = items.iter().map(|(track, _)| track.cast_unsigned()).collect();
        return Ok(Some((ids, version)));
    };
    // A row for each item, as the page lists them; every track has keys, checked above.
    let rows: Vec<SortRow> = items
        .iter()
        .map(|(track, added_at)| SortRow { added: rfc3339(*added_at), ..keys[track].clone() })
        .collect();
    let ids = sort_rows(&rows, field, descending).into_iter().map(|at| rows[at].id).collect();
    Ok(Some((ids, version)))
}

/// The tracks of `ids` that are in `library`, in the order given: one the search index still
/// holds that is gone since is left out.
fn in_library(conn: &Connection, library: i64, ids: &[i64]) -> rusqlite::Result<Vec<u64>> {
    by_ids(conn, &["t.id"], "tracks t", "", library, ids, |row| {
        row.get::<_, i64>(0).map(i64::cast_unsigned)
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::playlists::tests::behind_the_library_feed;

    /// What `recipe` resolves to on [`behind_the_library_feed`]'s device, for user 7 in library 1.
    fn resolved(recipe: Value) -> Result<Option<(Vec<u64>, Stored)>, ResolveError> {
        let conn = behind_the_library_feed();
        let recipe: Recipe = serde_json::from_value(recipe).unwrap();
        let tags = recipe.check().unwrap();
        resolve(&conn, 7, 1, &recipe, &tags, |_| Vec::new())
    }

    #[test]
    fn a_device_refuses_a_sorted_or_filtered_playlist_whose_track_has_not_arrived() {
        let sorted =
            json!({ "type": "playlist", "playlistId": "10", "sort": "name", "order": "asc" });
        let filtered =
            json!({ "type": "playlist", "playlistId": "10", "filters": { "tagAll": ["1"] } });
        for recipe in [sorted, filtered] {
            let result = resolved(recipe.clone());
            assert!(matches!(result, Err(ResolveError::Incomplete)), "{recipe}: {result:?}");
        }
        let plain = resolved(json!({ "type": "playlist", "playlistId": "10" })).unwrap().unwrap();
        assert_eq!(plain.0, [101, 102, 103], "its own order keeps every item in place");
    }
}
