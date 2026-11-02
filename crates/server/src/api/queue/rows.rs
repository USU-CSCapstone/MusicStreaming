//! `getQueueRows`: a page of the active queue's rows.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_core::Repeat;
use serde::{Deserialize, Serialize};

use crate::api::extract::Query;
use crate::api::scope::Caller;
use crate::api::{Id, Problem, Uuid};
use crate::db::Database;
use crate::queue::{Queues, Section, Slot};

/// The most rows a page holds, as many as the track lookup takes.
const MAX_ROWS: u64 = 100;

/// The spec's query for `getQueueRows`.
#[derive(Deserialize)]
pub struct RowsQuery {
    section: Section,
    #[serde(default)]
    offset: u64,
    limit: Option<u64>,
}

/// The spec's `QueueRow`: one row of a page. `track_id` is `null` where the track can't be read.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    item: String,
    library_id: String,
    track_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    play: Option<u64>,
}

/// The spec's `QueueRows`: a page of one section with the queue's counts and settings.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rows {
    queue_id: Uuid,
    version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<Row>,
    total: u64,
    manual: u64,
    shuffled: bool,
    repeat: Repeat,
    rows: Vec<Row>,
}

/// `limit` rows (up to 100, default 100) of the caller's active queue's `section` from
/// `offset`, newest first for History. `404` before a queue has been started. The positions are
/// taken under the queues' lock and the tracks read after it is released; a row whose list can't
/// be read has a `null` track and is unavailable.
pub async fn get_rows(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    caller: Caller,
    Query(query): Query<RowsQuery>,
) -> Result<Json<Rows>, Problem> {
    let limit = query.limit.unwrap_or(MAX_ROWS);
    if !(1..=MAX_ROWS).contains(&limit) {
        return Err(Problem::invalid("limit must be from 1 to 100"));
    }
    let page = queues
        .page(caller.user, query.section, query.offset, limit)
        .ok_or_else(Problem::not_found)?;
    let mut tracks = Queues::tracks(&db, caller.user, &page).await?.into_iter();
    let mut row = |slot: Slot| Row {
        track_id: tracks.next().flatten().map(|track| Id(track.cast_signed())),
        item: slot.item,
        library_id: slot.library_id,
        play: slot.play,
    };
    let current = page.current.map(&mut row);
    let rows = page.rows.into_iter().map(row).collect();
    Ok(Json(Rows {
        queue_id: page.queue_id,
        version: page.version,
        current,
        total: page.total,
        manual: page.manual,
        shuffled: page.shuffled,
        repeat: page.repeat,
        rows,
    }))
}
