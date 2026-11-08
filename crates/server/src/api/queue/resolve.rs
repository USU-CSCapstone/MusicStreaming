//! Recipes: resolving one into its track IDs (`design/queue.md` §2, §3). The recipe and the
//! resolution are the catalog's (`jewelcase_catalog::recipes`), which a device runs too; what is
//! here is the part only a server can do: the caller's reach of the library, and the search
//! index.

use std::sync::Arc;

pub use jewelcase_catalog::recipes::Recipe;
use jewelcase_catalog::recipes::resolve as run;
use jewelcase_core::{Kind, Recipe as Stored};
use rusqlite::params;

use super::refused;
use crate::api::scope::Caller;
use crate::api::search::Indexes;
use crate::api::tag_filter::TagFilter;
use crate::api::{Code, Id, Problem};
use crate::db::{Database, reaches};

/// The track IDs `recipe` resolves to in `library`, in order, with no paging (see
/// [`jewelcase_catalog::recipes::resolve`]). Refused, `422`, unless the caller reaches the
/// library, whether or not it exists, so no one learns what a library they cannot reach holds
/// (`requirements/users.md` §10). A recipe's filters can only be tags, at most `MAX_TAGS`, or the
/// call is refused, `422`. A playlist recipe resolves only the caller's own playlist in the
/// library: another account's, or one that does not exist, is `404`. A start or add must treat an
/// empty list as it does one sent. Gives the recipe as stored beside the IDs, which for a
/// playlist records the version read.
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
    // The index is the server's, read before the database is, since it may wait for a build.
    let ranked = match recipe {
        Recipe::Search { q, .. } => search(db, indexes, library, q.clone(), &tags).await?,
        _ => Vec::new(),
    };
    let (user, recipe) = (caller.user, recipe.clone());
    db.try_read(move |conn| {
        run(conn, user, library, &recipe, &tags, |_| ranked).map_err(Problem::from)
    })
    .await?
    .ok_or_else(Problem::not_found)
}

/// The IDs of every track of `library` that matches `q`, best first, as a search's tracks
/// section ranks them but with no limit, and only those that carry the tags `tags` names.
async fn search(
    db: &Arc<Database>,
    indexes: &Indexes,
    library: i64,
    q: String,
    tags: &TagFilter,
) -> Result<Vec<i64>, Problem> {
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
    Ok(found.into_iter().next().map(|section| section.ids).unwrap_or_default())
}
