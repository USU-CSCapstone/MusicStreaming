//! `Idempotency-Key` on every signed-in write (`api/openapi.yaml`).
//!
//! A POST, PUT, PATCH or DELETE that carries a key is carried out once for that account and key.
//! The answer of one that applied (a `2xx`) is stored, and a request that comes again with the
//! key, however late, gets the stored answer and changes nothing. A refused request changed
//! nothing, so it is not stored, and asking again with the same key is asked afresh. A key is
//! its account's own: nothing else is compared, so another account's use of it never meets this
//! one's. A key used again on another request gets the first one's answer. Answers are swept
//! after 8 days (`db/retention.rs`).
//!
//! The request's body is never held: the handler reads it as it arrives, and only the answer is
//! kept. Requests with the same key that overlap take turns, so the second finds the first's
//! answer. The work runs in a task of its own, so a client that gives up cannot stop it between
//! the change and the storing of its answer, which a retry then needs.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use axum::body::{Body, HttpBody, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rusqlite::{OptionalExtension, params};

use super::scope::Caller;
use super::{AppState, Code, Problem};
use crate::db::{Database, now_ms};

/// The longest key the spec allows, in characters.
const MAX_LEN: usize = 128;

/// The most an answer may weigh to be stored. A larger one, or one of unknown size, is sent
/// unstored; the writes' answers are small.
const MAX_ANSWER: usize = 1 << 20;

type Gate = Arc<tokio::sync::Mutex<()>>;

/// The requests being handled under a key, so that two with one key take turns.
#[derive(Default)]
pub struct Idempotency {
    gates: Mutex<HashMap<(i64, String), Gate>>,
}

/// The request's `Idempotency-Key`, if it sent one. A key that is empty, longer than the spec's
/// 128 characters, or not plain text is `422`.
pub fn key(headers: &HeaderMap) -> Result<Option<String>, Problem> {
    let Some(value) = headers.get("idempotency-key") else { return Ok(None) };
    match value.to_str() {
        Ok(key) if !key.is_empty() && key.len() <= MAX_LEN => Ok(Some(key.to_owned())),
        _ => Err(Problem::invalid(format!(
            "Idempotency-Key must be 1 to {MAX_LEN} characters of plain text."
        ))),
    }
}

/// The middleware of the signed-in routes: a write with an `Idempotency-Key` is handled by
/// [`Idempotency::once`] under the caller's key. Everything else passes through.
pub async fn layer(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if request.method().is_safe() {
        return next.run(request).await;
    }
    let key = match key(request.headers()) {
        Ok(Some(key)) => key,
        Ok(None) => return next.run(request).await,
        Err(problem) => return problem.into_response(),
    };
    let Some(caller) = request.extensions().get::<Caller>().copied() else {
        return next.run(request).await;
    };
    state.idempotency.once(state.db.clone(), caller.user, key, next.run(request)).await
}

impl Idempotency {
    /// The response of `work`, a write of `user`'s, carried out once for `key`: the stored answer
    /// if one is there, else `work`'s, which is stored if it applied. `work` never runs, and
    /// nothing changes, when a database error leaves it unknown whether it ran before.
    pub async fn once(
        self: &Arc<Self>,
        db: Arc<Database>,
        user: i64,
        key: String,
        work: impl Future<Output = Response> + Send + 'static,
    ) -> Response {
        let this = self.clone();
        let task = tokio::spawn(async move {
            let gate = this.gate(user, &key);
            let held = gate.lock().await;
            let response = match find(&db, user, &key).await {
                Ok(Some(stored)) => stored,
                Ok(None) => remember(&db, user, &key, work.await).await,
                Err(problem) => problem.into_response(),
            };
            drop(held);
            this.release(user, key, gate);
            response
        });
        task.await.unwrap_or_else(|_| Problem::new(Code::Internal).into_response())
    }

    /// The gate of `user`'s `key`, made if no request holds one.
    fn gate(&self, user: i64, key: &str) -> Gate {
        let mut gates = self.gates.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        gates.entry((user, key.to_owned())).or_default().clone()
    }

    /// Forgets `gate` once no other request waits on it.
    fn release(&self, user: i64, key: String, gate: Gate) {
        let mut gates = self.gates.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // The map's own and this request's.
        if Arc::strong_count(&gate) == 2 {
            gates.remove(&(user, key));
        }
    }
}

/// The answer stored under `user`'s `key`, as it was first sent.
async fn find(db: &Database, user: i64, key: &str) -> Result<Option<Response>, Problem> {
    let key = key.to_owned();
    let stored = db
        .read(move |conn| {
            conn.prepare_cached(
                "SELECT status, content_type, body FROM idempotency_keys \
                 WHERE user_id = ?1 AND key = ?2",
            )?
            .query_row(params![user, key], |row| {
                Ok((row.get::<_, u16>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })
            .optional()
        })
        .await?;
    Ok(stored.map(|(status, content_type, body)| {
        let mut response = Response::new(Body::from(body));
        // A status is stored only from one this server sent.
        *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
        if let Ok(value) = HeaderValue::from_str(&content_type)
            && !content_type.is_empty()
        {
            response.headers_mut().insert(header::CONTENT_TYPE, value);
        }
        response
    }))
}

/// `response`, after storing it under `user`'s `key` if it is a success small enough to keep.
/// A failure to store is logged: the write applied, so its answer is still sent.
async fn remember(db: &Database, user: i64, key: &str, response: Response) -> Response {
    if !response.status().is_success() {
        return response;
    }
    let (parts, body) = response.into_parts();
    if body.size_hint().upper().is_none_or(|size| size > MAX_ANSWER as u64) {
        return Response::from_parts(parts, body);
    }
    let Ok(bytes) = to_bytes(body, MAX_ANSWER).await else {
        return Problem::new(Code::Internal).into_response();
    };
    if let Ok(text) = std::str::from_utf8(&bytes) {
        let content_type = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let (key, text, status) = (key.to_owned(), text.to_owned(), parts.status.as_u16());
        let stored = db.write(move |tx| {
            tx.prepare_cached(
                "INSERT OR IGNORE INTO idempotency_keys \
                 (user_id, key, status, content_type, body, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![user, key, status, content_type, text, now_ms()])
            .map(drop)
        });
        if let Err(error) = stored.await {
            tracing::warn!(%error, "cannot store the answer of a write");
        }
    }
    Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
mod tests;
