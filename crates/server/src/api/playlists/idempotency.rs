//! `Idempotency-Key` on `createPlaylist` and `editPlaylist` (`api/openapi.yaml`).
//!
//! A request that applied is stored under the caller's key, in the transaction that applied it,
//! so a retry after a lost answer gets the stored answer and the change is not made twice. Only
//! answers of requests that applied are stored: a refused request changed nothing, so asking
//! again, with the same key, is asked afresh. Keys are the caller's own, and nothing else is
//! compared, so a key used again on another request gets the first one's answer. They are swept
//! after a day (`db/retention.rs`).

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::api::Problem;
use crate::api::problem::Code;

/// The longest key the spec allows, in characters.
const MAX_LEN: usize = 128;

/// The request's `Idempotency-Key`, if it sent one. A key that is empty, longer than the spec's
/// 128 characters, or not plain text is `422`.
pub struct Key(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for Key {
    type Rejection = Problem;

    /// Reads the `Idempotency-Key` header, if any, and checks its length and encoding.
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Problem> {
        let Some(value) = parts.headers.get("idempotency-key") else {
            return Ok(Key(None));
        };
        match value.to_str() {
            Ok(key) if !key.is_empty() && key.len() <= MAX_LEN => Ok(Key(Some(key.to_owned()))),
            _ => Err(Problem::invalid(format!(
                "Idempotency-Key must be 1 to {MAX_LEN} characters of plain text."
            ))),
        }
    }
}

/// A response of a create or an edit, as it is stored and sent: a status and a JSON body.
pub struct Answer {
    status: StatusCode,
    body: String,
}

impl Answer {
    /// `value` serialized, to be sent with `status`.
    pub fn new(status: StatusCode, value: &impl Serialize) -> Result<Answer, Problem> {
        match serde_json::to_string(value) {
            Ok(body) => Ok(Answer { status, body }),
            Err(error) => {
                tracing::error!(%error, "cannot serialize a playlist");
                Err(Problem::new(Code::Internal))
            }
        }
    }

    /// The answer as a response, with the JSON content type.
    pub fn into_response(self) -> Response {
        (self.status, [(header::CONTENT_TYPE, "application/json")], self.body).into_response()
    }

    /// The answer stored under `user`'s `key`, if the request had a key and there is one.
    pub fn find(
        conn: &Connection,
        user: i64,
        key: Option<&str>,
    ) -> rusqlite::Result<Option<Answer>> {
        let Some(key) = key else { return Ok(None) };
        let stored = conn
            .prepare_cached(
                "SELECT status, body FROM idempotency_keys WHERE user_id = ?1 AND key = ?2",
            )?
            .query_row(params![user, key], |row| Ok((row.get::<_, u16>(0)?, row.get(1)?)))
            .optional()?;
        // A status is stored only from one this server sent.
        Ok(stored.map(|(status, body)| Answer {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
            body,
        }))
    }

    /// Stores the answer under `user`'s `key`, made at `now`. Call it in the transaction of the
    /// change, after [`find`](Self::find) found nothing in it.
    pub fn store(&self, conn: &Connection, user: i64, key: &str, now: i64) -> rusqlite::Result<()> {
        conn.prepare_cached(
            "INSERT INTO idempotency_keys (user_id, key, status, body, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?
        .execute(params![user, key, self.status.as_u16(), self.body, now])
        .map(drop)
    }
}
