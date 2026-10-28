//! Starting a queue from track IDs: `startQueue`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use jewelcase_core::{Queue, Source};
use rusqlite::params;
use serde::Deserialize;
use serde_json::Value;

use super::{Account, Queues};
use crate::api::scope::Caller;
use crate::api::search::{self, Indexes};
use crate::api::tracks::{self, ListQuery};
use crate::api::{Id, Problem, Uuid, extract, plays};
use crate::db::{self, Database, reaches};

/// The spec's `StartQueue`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Start {
    queue_id: Uuid,
    context: Value,
    source: NewSource,
    /// Where a source of `tracks` starts.
    start: Option<usize>,
    /// The track a source of a `query` starts at, at its first place in the list.
    start_track_id: Option<String>,
    shuffle: Option<Shuffle>,
}

/// The source a queue starts from: the tracks as the device resolved them, or the list query
/// the server is to run. Exactly one of the two.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewSource {
    key: Uuid,
    library_id: String,
    tracks: Option<Vec<String>>,
    query: Option<ViewQuery>,
}

/// A list a view pages through, which the server reads to its end.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum ViewQuery {
    /// The parameters of `listTracks`; `cursor`, `limit` and `ids` are refused.
    Tracks(ListQuery),
    /// A search's track section, all of it and not a `sectionLimit` of it.
    Search { q: String },
}

/// Start with the context shuffled.
#[derive(Deserialize)]
struct Shuffle {
    seed: u32,
}

/// Replaces the caller's queue with one that plays the request's source from its start,
/// carrying the Manual Queue and the repeat mode over. Refuses the request, `422`, unless every
/// track is in the library and the caller reaches the library, whether or not what is refused
/// exists, so no one learns what a library they cannot reach holds (`requirements/users.md`
/// §10). A source that is a `query` is run to its end by the server, in the order the list
/// pages in, and refused if the start track is not in the result. A request for the queue the
/// caller already has, by `queueId`, changes nothing, so a device may send it again.
pub async fn start(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    State(indexes): State<Arc<Indexes>>,
    caller: Caller,
    extract::Json(request): extract::Json<Start>,
) -> Result<StatusCode, Problem> {
    // The context is checked as a play's is, and kept as sent.
    plays::json_text(&request.context, "A queue's context", true)?;
    let library = Id::canonical(&request.source.library_id).ok_or_else(refused)?;

    let NewSource { key, library_id, tracks, query } = request.source;
    let (tracks, start) = match (tracks, query, request.start, request.start_track_id) {
        (Some(tracks), None, Some(start), None) => {
            check_tracks(&db, &caller, library, &tracks).await?;
            (tracks, start)
        }
        (None, Some(query), None, Some(start_track)) => {
            let tracks = run(&db, &indexes, &caller, library, query).await?;
            let start = tracks.iter().position(|track| *track == start_track);
            let start =
                start.ok_or_else(|| Problem::invalid("startTrackId is not in the list."))?;
            (tracks, start)
        }
        _ => {
            return Err(Problem::invalid(
                "A source is either tracks, started at start, or a query, started at startTrackId.",
            ));
        }
    };

    let source = Source { key: key.as_str().to_owned(), library_id, tracks };
    let seed = request.shuffle.map(|shuffle| shuffle.seed);
    queues.replace(caller.user, request.queue_id, request.context, source, start, seed)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The refusal for a track out of the library or a library out of reach, the same whether or
/// not it exists.
fn refused() -> Problem {
    Problem::invalid("Every track must be in the library, and the library one you reach.")
}

/// Refuses, `422`, unless every one of `tracks` is in `library` and the caller reaches it.
async fn check_tracks(
    db: &Database,
    caller: &Caller,
    library: Id,
    tracks: &[String],
) -> Result<(), Problem> {
    let mut distinct = tracks
        .iter()
        .map(|track| Id::canonical(track).map(|Id(id)| id))
        .collect::<Option<Vec<i64>>>()
        .ok_or_else(refused)?;
    distinct.sort_unstable();
    distinct.dedup();

    // Counted by the database from the IDs as one JSON array, which takes one statement
    // however many there are.
    let ids = serde_json::to_string(&distinct).map_err(|_| refused())?;
    let (admin, user) = (caller.admin, caller.user);
    let found: usize = db
        .read(move |conn| {
            conn.prepare_cached(concat!(
                "SELECT count(*) FROM tracks t \
                 WHERE t.library_id = ?1 AND t.id IN (SELECT value FROM json_each(?2)) AND ",
                reaches!(admin: "?3", user: "?4", library: "?1")
            ))?
            .query_row(params![library.0, ids, admin, user], |row| row.get(0))
        })
        .await?;
    if found == distinct.len() { Ok(()) } else { Err(refused()) }
}

/// The IDs of the tracks `query` finds in `library`, in order, as text. Refuses, `422`, a
/// library that does not exist or the caller does not reach, like a track outside it. Every
/// track is in the library, since the query reads only its tracks, and the ones a search knew
/// of that are gone are left out, as a search's results leave them.
async fn run(
    db: &Arc<Database>,
    indexes: &Indexes,
    caller: &Caller,
    library: Id,
    query: ViewQuery,
) -> Result<Vec<String>, Problem> {
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

    let ids: Vec<i64> = match query {
        ViewQuery::Tracks(list) => {
            list.check_whole()?;
            db.read(move |conn| tracks::all_ids(conn, library.0, list)).await?
        }
        ViewQuery::Search { q } => {
            let found = search::track_ids(db, indexes, library.0, &q).await?;
            db.read(move |conn| {
                db::by_ids(conn, &["t.id"], "tracks t", "", library.0, &found, |row| row.get(0))
            })
            .await?
        }
    };
    Ok(ids.iter().map(i64::to_string).collect())
}

impl Queues {
    /// Makes `user`'s queue one that plays `source` from `start`, shuffled with `seed` if given,
    /// and keeps the Manual Queue and repeat mode they had. Does nothing if their queue is `queue_id` already.
    pub(super) fn replace(
        &self,
        user: i64,
        queue_id: Uuid,
        context: Value,
        source: Source,
        start: usize,
        seed: Option<u32>,
    ) -> Result<(), Problem> {
        let invalid = |error: jewelcase_core::QueueError| Problem::invalid(error.to_string());
        let mut accounts = self.lock();
        let before = accounts.get(&user);
        if before.is_some_and(|account| account.queue_id == queue_id) {
            return Ok(());
        }
        let queue = Queue::launch(source, start, seed, before.map(|account| &account.queue))
            .map_err(invalid)?;
        // None of this queue is in the database, whatever an earlier queue left there, so the
        // next save replaces it.
        let saved = None;
        accounts
            .insert(user, Account { queue_id, version: 0, context, queue, unsaved: true, saved });
        Ok(())
    }
}
