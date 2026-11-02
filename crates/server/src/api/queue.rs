//! The account's queue: `getQueue` and `startQueue` (`requirements/queue.md`,
//! `design/queue.md` §9).
//!
//! The queue itself lives in [`crate::queue`]; this is its API. A device that starts a queue
//! chooses its `queueId` and its source's key, so it can apply the start at once, and the server
//! checks that the caller reaches every track the source names.

mod start;

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::Problem;
use super::scope::Caller;
use crate::queue::Queues;
pub use start::start;

/// The largest body a start accepts: about 2.9 million track IDs, far past any library the
/// server is meant for, and still bounded so one request cannot exhaust a small host's memory.
/// Past it the body is refused as `422 validation_failed`.
pub const BODY_LIMIT: usize = 64 << 20;

/// `getQueue`: the caller's own queue, or `404` before they have started one. It carries an
/// `ETag` naming the server's start, the queue and its version, and `304` with no body answers
/// an `If-None-Match` that names it, so a device that reconnects downloads nothing it has.
pub async fn get(
    State(queues): State<Arc<Queues>>,
    caller: Caller,
    headers: HeaderMap,
) -> Result<Response, Problem> {
    let known = headers.get(header::IF_NONE_MATCH).and_then(|value| value.to_str().ok());
    let (etag, body) = queues.snapshot(caller.user, known).ok_or_else(Problem::not_found)?;
    Ok(match body {
        Some(body) => ([(header::CONTENT_TYPE, "application/json"), (header::ETAG, &etag)], body)
            .into_response(),
        None => (StatusCode::NOT_MODIFIED, [(header::ETAG, &etag)]).into_response(),
    })
}

#[cfg(test)]
mod tests;
