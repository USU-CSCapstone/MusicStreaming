//! Who is calling: the device a request's token belongs to (`bearerAuth` and `cookieAuth` in
//! `api/openapi.yaml`), and whether it may reach the library its path names.
//!
//! A request carries its token as `Authorization: Bearer <token>` or, from the web app, in the
//! session cookie. A browser sends that cookie by itself, so a cookie-authenticated write must
//! also carry `X-Jewelcase-Client`, which a page on another site cannot add without asking.
//!
//! A library the caller cannot reach answers `404` for everything under it, exactly as one that
//! does not exist (`requirements/users.md` §10). Admins and the owner reach every library;
//! anyone else only those granted to them (§5). Handlers below `/libraries/{library_id}` can
//! rely on that, and on nothing the client says.

use std::time::Duration;

use axum::extract::rejection::RawPathParamsRejection;
use axum::extract::{FromRequestParts, RawPathParams, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension};

use super::session::{COOKIE, token_hash};
use super::{AppState, Code, Id, Problem};
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
    /// An admin or the owner: administers the server, and reaches every library.
    pub admin: bool,
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Caller, Problem> {
        parts.extensions.get::<Caller>().copied().ok_or_else(|| Problem::new(Code::Unauthenticated))
    }
}

/// Answers `401 unauthenticated` unless the request carries the token of a device whose
/// account is active, `403` if it is under `/admin` and the account is not an admin, and `404`
/// if its path names a library that account cannot reach.
/// Otherwise it passes the [`Caller`] on to the handler.
pub async fn require(
    State(state): State<AppState>,
    params: Result<RawPathParams, RawPathParamsRejection>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some((token, from_cookie)) = token(request.headers()) else {
        return unauthenticated();
    };
    let hash = token_hash(token);
    let library = library(params);
    let found = match state.db.read(move |conn| device(conn, &hash, library)).await {
        Ok(found) => found,
        Err(error) => return Problem::from(error).into_response(),
    };
    let Some(Found { caller, last_seen_at, reaches_library }) = found else {
        return unauthenticated();
    };
    // Checked once the token is known to be good, so a bad one is always `401`.
    if from_cookie && !request.method().is_safe() && !request.headers().contains_key(CLIENT) {
        return Problem::new(Code::Forbidden)
            .detail("A write authenticated by cookie must send X-Jewelcase-Client.")
            .into_response();
    }
    // Everything under `/admin` is for admins and the owner (`requirements/users.md` §1). A user
    // is told they may not, before anything the path names is looked up (§10).
    if !caller.admin && request.uri().path().starts_with("/admin/") {
        return Problem::new(Code::Forbidden).into_response();
    }
    if !reaches_library {
        return Problem::not_found().into_response();
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

/// The library the path names, if any. One it names badly is `-1`, which no library is.
fn library(params: Result<RawPathParams, RawPathParamsRejection>) -> Option<i64> {
    let Ok(params) = params else { return Some(-1) };
    let (_, id) = params.iter().find(|(name, _)| *name == "library_id")?;
    Some(Id::canonical(id).map_or(-1, |Id(id)| id))
}

struct Found {
    caller: Caller,
    last_seen_at: i64,
    /// Whether the caller reaches the library the path names; true if it names none.
    reaches_library: bool,
}

/// The caller behind `hash`, if it is a token of an active account, and whether it reaches
/// `library`. A suspended account's tokens answer exactly as unknown ones do
/// (`requirements/users.md` §10).
fn device(
    conn: &Connection,
    hash: &[u8; 32],
    library: Option<i64>,
) -> rusqlite::Result<Option<Found>> {
    conn.prepare_cached(
        "SELECT d.id, d.user_id, d.last_seen_at, u.role <> 'user', CASE \
             WHEN ?2 IS NULL THEN 1 \
             WHEN u.role <> 'user' THEN EXISTS (SELECT 1 FROM libraries WHERE id = ?2) \
             ELSE EXISTS (SELECT 1 FROM library_access WHERE user_id = u.id AND library_id = ?2) \
         END \
         FROM devices d JOIN users u ON u.id = d.user_id \
         WHERE d.token_hash = ?1 AND u.status = 'active'",
    )?
    .query_row(rusqlite::params![hash, library], |row| {
        Ok(Found {
            caller: Caller { device: row.get(0)?, user: row.get(1)?, admin: row.get(3)? },
            last_seen_at: row.get(2)?,
            reaches_library: row.get(4)?,
        })
    })
    .optional()
}

/// A `401`, with the challenge RFC 9110 asks every `401` to name.
fn unauthenticated() -> Response {
    let challenge = [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))];
    (challenge, Problem::new(Code::Unauthenticated)).into_response()
}

#[cfg(test)]
mod tests;
