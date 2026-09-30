//! Who is calling: the device a request's token belongs to (`bearerAuth` and `cookieAuth` in
//! `api/openapi.yaml`).
//!
//! A request carries its token as `Authorization: Bearer <token>` or, from the web app, in the
//! session cookie. A browser sends that cookie by itself, so a cookie-authenticated write must
//! also carry `X-Jewelcase-Client`, which a page on another site cannot add without asking.

use std::time::Duration;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension};

use super::session::{COOKIE, token_hash};
use super::{AppState, Code, Problem};
use crate::db::now_ms;

/// The header a cookie-authenticated write must carry.
const CLIENT: &str = "x-jewelcase-client";

/// How stale a device's `last_seen_at` may get before a request refreshes it. Coarse, so that
/// using a device does not cost a write per request.
const LAST_SEEN_EVERY: Duration = Duration::from_secs(5 * 60);

/// The account and device a request is from. Handlers take it as an argument; only routes
/// behind [`require`] can produce one.
#[derive(Debug, Clone, Copy)]
pub struct Caller {
    pub user: i64,
    pub device: i64,
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Caller, Problem> {
        parts.extensions.get::<Caller>().copied().ok_or_else(|| Problem::new(Code::Unauthenticated))
    }
}

/// Answers `401 unauthenticated` unless the request carries the token of a device whose
/// account is active, and otherwise passes the [`Caller`] on to the handler.
pub async fn require(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let Some((token, from_cookie)) = token(request.headers()) else {
        return unauthenticated();
    };
    let hash = token_hash(token);
    let found = match state.db.read(move |conn| device(conn, &hash)).await {
        Ok(found) => found,
        Err(error) => return Problem::from(error).into_response(),
    };
    let Some((caller, last_seen_at)) = found else {
        return unauthenticated();
    };
    // Checked once the token is known to be good, so a bad one is always `401`.
    if from_cookie && !request.method().is_safe() && !request.headers().contains_key(CLIENT) {
        return Problem::new(Code::Forbidden)
            .detail("A write authenticated by cookie must send X-Jewelcase-Client.")
            .into_response();
    }
    let stale_before = now_ms() - LAST_SEEN_EVERY.as_millis() as i64;
    if last_seen_at < stale_before {
        // Off the request's path: a busy writer must not hold up what the user asked for. The
        // requests that arrive together all find it stale, so it is checked again in the write:
        // after the first, each changes nothing, and SQLite commits those without touching disk.
        let db = state.db.clone();
        tokio::spawn(async move {
            let touch = db.write(move |tx| {
                tx.execute(
                    "UPDATE devices SET last_seen_at = ?2 WHERE id = ?1 AND last_seen_at < ?3",
                    [caller.device, now_ms(), stale_before],
                )
            });
            if let Err(error) = touch.await {
                tracing::warn!(%error, "cannot record when a device was last seen");
            }
        });
    }
    request.extensions_mut().insert(caller);
    next.run(request).await
}

/// The request's token, and whether it came from the cookie. A bearer token wins over a cookie.
fn token(headers: &HeaderMap) -> Option<(&str, bool)> {
    if let Some(authorization) = headers.get(header::AUTHORIZATION) {
        let (scheme, token) = authorization.to_str().ok()?.split_once(' ')?;
        return scheme.eq_ignore_ascii_case("bearer").then_some((token.trim(), false));
    }
    let cookies = headers.get_all(header::COOKIE).into_iter();
    let mut pairs =
        cookies.filter_map(|value| value.to_str().ok()).flat_map(|value| value.split(';'));
    let token = pairs.find_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))?;
    Some((token, true))
}

/// The caller and when its device was last seen, if `hash` is a token of an active account.
/// A suspended account's tokens answer exactly as unknown ones do (`requirements/users.md` §10).
fn device(conn: &Connection, hash: &[u8; 32]) -> rusqlite::Result<Option<(Caller, i64)>> {
    conn.prepare_cached(
        "SELECT d.id, d.user_id, d.last_seen_at FROM devices d JOIN users u ON u.id = d.user_id \
         WHERE d.token_hash = ?1 AND u.status = 'active'",
    )?
    .query_row([hash], |row| Ok((Caller { device: row.get(0)?, user: row.get(1)? }, row.get(2)?)))
    .optional()
}

/// A `401`, with the challenge RFC 9110 asks every `401` to name.
fn unauthenticated() -> Response {
    let challenge = [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))];
    (challenge, Problem::new(Code::Unauthenticated)).into_response()
}

#[cfg(test)]
mod tests;
