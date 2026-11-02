//! `getQueueList`: one of the caller's lists, by hash.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::api::Problem;
use crate::api::scope::Caller;
use crate::db::Database;
use crate::queue::Queues;

/// The list `hash` of one of the caller's queues, active, kept or not yet saved, as
/// `{libraryId, tracks}` with IDs as decimal strings. A hash no queue of theirs uses, or none at
/// all, is `404`, so no one learns what another account holds. Reach is not checked again: every
/// track was checked as it joined a queue. A list never changes under its hash, so the answer is
/// cached for good, and an `If-None-Match` naming it is answered `304` once the caller is known
/// to hold it.
pub async fn get_list(
    State(db): State<Arc<Database>>,
    State(queues): State<Arc<Queues>>,
    caller: Caller,
    Path(hash): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Problem> {
    let etag = format!("\"{hash}\"");
    let known = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|known| crate::queue::names(known, &etag));
    let body =
        queues.list_json(&db, caller.user, &hash, !known).await?.ok_or_else(Problem::not_found)?;
    let etag = HeaderValue::from_str(&etag).map_err(|_| Problem::not_found())?;
    let cache = HeaderValue::from_static("private, max-age=31536000, immutable");
    let headers = [(header::ETAG, etag), (header::CACHE_CONTROL, cache)];
    Ok(if known {
        (StatusCode::NOT_MODIFIED, headers).into_response()
    } else {
        ([(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))], headers, body)
            .into_response()
    })
}
