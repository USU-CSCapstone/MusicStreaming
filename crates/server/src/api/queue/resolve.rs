//! Recipes: how a list is made, and resolving one into its track IDs (`design/queue.md` §2, §3).
//!
//! A start, and later an add, names a recipe instead of sending tracks. The server runs it
//! against the library, in the caller's reach, to the same order a view of it pages in to its
//! end. The core keeps a recipe's fields as text ([`jewelcase_core::Recipe`]); here they are
//! typed as the API needs them.

use std::sync::Arc;

use jewelcase_core::{Kind, Recipe as Stored};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::refused;
use crate::api::credit::ArtistCredit;
use crate::api::page::Order;
use crate::api::scope::Caller;
use crate::api::search::Indexes;
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
    /// An album's tracks in a sort.
    Album { album_id: String, sort: TrackSort, order: Order },
    /// An artist's tracks, credited as `artist_credit` says, in a sort.
    Artist {
        artist_id: String,
        scope: Scope,
        artist_credit: ArtistCredit,
        sort: TrackSort,
        order: Order,
    },
    /// A library's tracks under a browse filter, in a sort. No filter is supported yet, so
    /// `filters` must be empty.
    Library { filters: Map<String, Value>, sort: TrackSort, order: Order },
    /// The tracks a search finds, in its ranking.
    Search { q: String },
}

/// Which of an artist's tracks a recipe covers. Only `all` is supported so far, since the other
/// scopes of an artist's page are albums, or a personal ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    All,
}

impl Recipe {
    /// The recipe as the core keeps it, which is how a snapshot shows it.
    pub fn stored(&self) -> Stored {
        match self {
            Recipe::Album { album_id, sort, order } => {
                let (sort, order) = names(*sort, *order);
                Stored::Album { album_id: album_id.clone(), sort, order }
            }
            Recipe::Artist { artist_id, artist_credit, sort, order, .. } => {
                let (sort, order) = names(*sort, *order);
                Stored::Artist {
                    artist_id: artist_id.clone(),
                    scope: "all".to_owned(),
                    artist_credit: artist_credit.name().to_owned(),
                    sort,
                    order,
                }
            }
            Recipe::Library { filters, sort, order } => {
                let (sort, order) = names(*sort, *order);
                Stored::Library { filters: filters.clone(), sort, order }
            }
            Recipe::Search { q } => Stored::Search { q: q.clone() },
        }
    }

    /// Refuses, `422`, what the server cannot resolve yet.
    fn check(&self) -> Result<(), Problem> {
        match self {
            Recipe::Library { filters, .. } if !filters.is_empty() => {
                Err(Problem::invalid("A library recipe supports no filters yet."))
            }
            Recipe::Search { q } if q.is_empty() => Err(Problem::invalid("q must not be empty")),
            _ => Ok(()),
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
/// An album or artist the library lacks resolves to no tracks. A start or add must treat an empty
/// list as it does one sent.
pub async fn resolve(
    db: &Arc<Database>,
    indexes: &Indexes,
    caller: &Caller,
    library: Id,
    recipe: &Recipe,
) -> Result<Vec<u64>, Problem> {
    recipe.check()?;
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
    let ids = match recipe.clone() {
        Recipe::Album { album_id, sort, order } => {
            let album = Some(AnyId::from(album_id.as_str()));
            let sort = TrackOrder { sort, order };
            db.read(move |conn| {
                tracks::resolve(conn, library, album, None, ArtistCredit::Any, sort)
            })
            .await?
        }
        Recipe::Artist { artist_id, artist_credit, sort, order, scope: Scope::All } => {
            let artist = Some(AnyId::from(artist_id.as_str()));
            let sort = TrackOrder { sort, order };
            db.read(move |conn| tracks::resolve(conn, library, None, artist, artist_credit, sort))
                .await?
        }
        Recipe::Library { sort, order, .. } => {
            let sort = TrackOrder { sort, order };
            db.read(move |conn| tracks::resolve(conn, library, None, None, ArtistCredit::Any, sort))
                .await?
        }
        Recipe::Search { q } => search(db, indexes, library, q).await?,
    };
    Ok(ids)
}

/// Every track of `library` that matches `q`, best first, as a search's tracks section ranks
/// them but with no limit. Those the index holds that are gone since are left out.
async fn search(
    db: &Arc<Database>,
    indexes: &Indexes,
    library: i64,
    q: String,
) -> Result<Vec<u64>, Problem> {
    let index = indexes.index(db, library).await?;
    // Ranking every match of a common word takes a while, so it runs off the async threads.
    let found = tokio::task::spawn_blocking(move || index.search(&q, &[Kind::Track], usize::MAX))
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
