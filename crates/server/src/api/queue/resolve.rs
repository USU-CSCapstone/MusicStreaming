//! Recipes: how a list is made, and resolving one into its track IDs (`design/queue.md` §2, §3).
//!
//! A start, and later an add, names a recipe instead of sending tracks. The server runs it
//! against the library, in the caller's reach, to the same order a view of it pages in to its
//! end. The core keeps a recipe's fields as text ([`jewelcase_core::Recipe`]); here they are
//! typed as the API needs them.

use std::sync::Arc;

use jewelcase_core::{Kind, Recipe as Stored, SortField, SortRow, sort_rows};
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Map, Value};

use super::refused;
use crate::api::credit::ArtistCredit;
use crate::api::page::Order;
use crate::api::playlists;
use crate::api::scope::Caller;
use crate::api::search::Indexes;
use crate::api::tag_filter::{TagFilter, Tags};
use crate::api::time::rfc3339;
use crate::api::tracks::{self, TrackOrder, TrackSort};
use crate::api::{AnyId, Code, Id, Problem};
use crate::db::{Database, by_ids, reaches};

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
/// `tagAny` name them. Any other filter is refused, `422`.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct Filters {
    tag_all: Vec<String>,
    tag_any: Vec<String>,
}

/// The tag filter `filters` names, as a list reads it: refused, `422`, when it holds any other
/// filter, or more than `MAX_TAGS` tags.
fn tag_filter(filters: &Map<String, Value>) -> Result<TagFilter, Problem> {
    let Filters { tag_all, tag_any } = serde_json::from_value(Value::Object(filters.clone()))
        .map_err(|e| Problem::invalid(format!("A recipe's filters can only be tags: {e}")))?;
    let join = |ids: Vec<String>| (!ids.is_empty()).then(|| ids.join(","));
    TagFilter::new(join(tag_all).as_deref(), join(tag_any).as_deref())
}

impl Recipe {
    /// The recipe as the core keeps it, which is how a snapshot shows it. A playlist's has no
    /// version until [`resolve`] gives it one.
    fn stored(&self) -> Stored {
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

    /// Refuses, `422`, what the server cannot resolve, and gives the tag filter.
    fn check(&self) -> Result<TagFilter, Problem> {
        match self {
            Recipe::Search { q, .. } if q.is_empty() => {
                Err(Problem::invalid("q must not be empty"))
            }
            Recipe::Playlist { sort, order, .. } if sort.is_some() != order.is_some() => {
                Err(Problem::invalid("A playlist recipe's sort and order come together."))
            }
            Recipe::Album { filters, .. }
            | Recipe::Playlist { filters, .. }
            | Recipe::Artist { filters, .. }
            | Recipe::Library { filters, .. }
            | Recipe::Search { filters, .. } => tag_filter(filters),
        }
    }
}

/// `sort` and `order` as a stored recipe spells them.
fn names(sort: TrackSort, order: Order) -> (String, String) {
    let (sort, order) = TrackOrder { sort, order }.names();
    (sort.to_owned(), order.to_owned())
}

/// The track IDs `recipe` resolves to in `library`, in order, one statement for each part of the
/// sort's index and read straight into `u64`, with no paging. Refused, `422`, unless the caller
/// reaches the library, whether or not it exists, so no one learns what a library they cannot
/// reach holds (`requirements/users.md` §10). Every track is in the library, since a recipe
/// reads only its tracks, and a search leaves out the ones its index still holds that are gone.
/// An album, artist or tag the library lacks resolves to no tracks. A recipe's filters can only be
/// tags, at most `MAX_TAGS`, or the call is refused, `422`. A playlist recipe resolves only
/// the caller's own playlist in the library, as the playlist endpoints find it: another account's,
/// or one that does not exist, is `404`. A start or add must treat an empty list as it does one
/// sent. Gives the recipe as stored beside the IDs, which for a playlist records the version read.
pub async fn resolve(
    db: &Arc<Database>,
    indexes: &Indexes,
    caller: &Caller,
    library: Id,
    recipe: &Recipe,
) -> Result<(Vec<u64>, Stored), Problem> {
    let tags = recipe.check()?;
    let (admin, user) = (caller.admin, caller.user);
    let reached: bool = db
        .read(move |conn| {
            conn.prepare_cached(concat!(
                "SELECT EXISTS (SELECT 1 FROM libraries l WHERE l.id = ?1 AND ",
                reaches!(admin: "?2", user: "?3", library: "l.id"),
                ")"
            ))?
            .query_row(params![library.0, admin, user], |row| row.get(0))
        })
        .await?;
    if !reached {
        return Err(refused());
    }

    let library = library.0;
    let mut stored = recipe.stored();
    let ids = match recipe.clone() {
        Recipe::Album { album_id, sort, order, .. } => {
            let album = Some(AnyId::from(album_id.as_str()));
            let sort = TrackOrder { sort, order };
            db.read(move |conn| {
                tracks::resolve(conn, library, album, None, ArtistCredit::Any, &tags, sort)
            })
            .await?
        }
        Recipe::Artist { artist_id, artist_credit, sort, order, scope: Scope::All, .. } => {
            let artist = Some(AnyId::from(artist_id.as_str()));
            let sort = TrackOrder { sort, order };
            db.read(move |conn| {
                tracks::resolve(conn, library, None, artist, artist_credit, &tags, sort)
            })
            .await?
        }
        Recipe::Library { sort, order, .. } => {
            let sort = TrackOrder { sort, order };
            db.read(move |conn| {
                tracks::resolve(conn, library, None, None, ArtistCredit::Any, &tags, sort)
            })
            .await?
        }
        Recipe::Search { q, .. } => search(db, indexes, library, q, &tags).await?,
        Recipe::Playlist { playlist_id, sort, order, .. } => {
            let playlist = AnyId::from(playlist_id.as_str()).0.ok_or_else(Problem::not_found)?;
            let sort = sort.map(|sort| (sort, order == Some(Order::Desc)));
            let user = caller.user;
            let (ids, read) = db
                .try_read(move |conn| playlist_ids(conn, user, library, playlist, sort, &tags))
                .await?;
            if let Stored::Playlist { version, .. } = &mut stored {
                *version = Some(read);
            }
            ids
        }
    };
    Ok((ids, stored))
}

/// The IDs of the items of `user`'s playlist `playlist` in `library`, and the playlist's
/// version, from one read: its items in key order, a track as often as it holds it and a missing
/// track in place, kept if they carry the tags `tags` names, then in `sort`'s order if given
/// (`SortField` and whether descending) by the core's comparator over the keys the playlist's
/// page sorts by. `404` for a playlist that is not the user's own in the library.
fn playlist_ids(
    conn: &Connection,
    user: i64,
    library: i64,
    playlist: i64,
    sort: Option<(SortField, bool)>,
    tags: &TagFilter,
) -> Result<(Vec<u64>, i64), Problem> {
    let (version, mut items) = playlists::all_items(conn, user, library, playlist)?;
    let tagged = Tags::read(conn, library, &tracks::TAGGED, tags)?.ids(conn, &tracks::TAGGED)?;
    if let Some(tagged) = tagged {
        items.retain(|(track, _)| tagged.contains(track));
    }
    let Some((field, descending)) = sort else {
        return Ok((items.iter().map(|(track, _)| track.cast_unsigned()).collect(), version));
    };
    let mut wanted: Vec<i64> = items.iter().map(|(track, _)| *track).collect();
    wanted.sort_unstable();
    wanted.dedup();
    let keys = tracks::sort_keys(conn, library, &wanted)?;
    // A row for each item, as the page lists them; one whose track is gone has none to show.
    let rows: Vec<SortRow> = items
        .iter()
        .filter_map(|(track, added_at)| {
            Some(SortRow { added: rfc3339(*added_at), ..keys.get(track)?.clone() })
        })
        .collect();
    let ids = sort_rows(&rows, field, descending).into_iter().map(|at| rows[at].id).collect();
    Ok((ids, version))
}

/// Every track of `library` that matches `q`, best first, as a search's tracks section ranks
/// them but with no limit, and only those that carry the tags `tags` names. Those the index holds
/// that are gone since are left out.
async fn search(
    db: &Arc<Database>,
    indexes: &Indexes,
    library: i64,
    q: String,
    tags: &TagFilter,
) -> Result<Vec<u64>, Problem> {
    let tagged = indexes.tagged.get(db, library, tags, &[Kind::Track]).await?;
    let index = indexes.index(db, library).await?;
    // Ranking every match of a common word takes a while, so it runs off the async threads.
    let found = tokio::task::spawn_blocking(move || {
        index.search_within(&q, &[Kind::Track], usize::MAX, |kind, id| {
            tagged.as_ref().is_none_or(|tagged| tagged[kind as usize].contains(&id))
        })
    })
    .await
    .map_err(|_| Problem::new(Code::Internal))?;
    let ids = found.into_iter().next().map(|section| section.ids).unwrap_or_default();
    let ids = db
        .read(move |conn| {
            by_ids(conn, &["t.id"], "tracks t", "", library, &ids, |row| {
                row.get::<_, i64>(0).map(i64::cast_unsigned)
            })
        })
        .await?;
    Ok(ids)
}
