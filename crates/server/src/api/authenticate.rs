//! Who is calling: the device a request's token belongs to (`bearerAuth` and `cookieAuth` in
//! `api/openapi.yaml`), and whether it may reach the library its path names.
//!
//! A request carries its token as `Authorization: Bearer <token>` or, from the web app, in the
//! session cookie. A browser sends that cookie by itself, so a cookie-authenticated write must
//! also carry `X-Jewelcase-Client`, which a page on another site cannot add without asking.
//!
//! A device signed out by anything but its own `logout` keeps its token hash, so a request
//! with that token answers `401 session_revoked` with the reason, which tells the device
//! whether to keep its downloads (`requirements/offline.md` §9).
//!
//! A library the caller cannot reach answers `404` for everything under it, exactly as one that
//! does not exist (`requirements/users.md` §10). Admins and the owner reach every library;
//! anyone else only those granted to them (§5). A handler whose path names a library gets it
//! only from the [`Scope`] made here, once that reach is checked.

use std::time::Duration;

use axum::extract::rejection::RawPathParamsRejection;
use axum::extract::{RawPathParams, Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension};

use super::realtime::Revoked;
use super::scope::{Caller, Scope};
use super::session::{self, COOKIE, token_hash};
use super::{AnyId, AppState, Code, Problem};
use crate::db::{Text, now_ms, reaches};

/// The header a cookie-authenticated write must carry.
const CLIENT: &str = "x-jewelcase-client";

/// How stale a device's `last_seen_at` may get before a request refreshes it. Coarse, so that
/// using a device does not cost a write per request.
const LAST_SEEN_EVERY: Duration = Duration::from_secs(5 * 60);

/// Answers `401 unauthenticated` unless the request carries the token of a device whose
/// account is active, `401 session_revoked` if that device is signed out, and `404` if its path
/// names a library that account cannot reach. Otherwise it passes the [`Caller`] on to the
/// handler, and the [`Scope`] if its path names a library.
pub async fn require(
    State(state): State<AppState>,
    params: Result<RawPathParams, RawPathParamsRejection>,
    request: Request,
    next: Next,
) -> Response {
    gate(state, params, request, next, false).await
}

/// [`require`] for the routes under `/admin`, which also answers `403` unless the account is an
/// admin or the owner (`requirements/users.md` §1), whatever the path names (§10).
pub async fn require_admin(
    State(state): State<AppState>,
    params: Result<RawPathParams, RawPathParamsRejection>,
    request: Request,
    next: Next,
) -> Response {
    gate(state, params, request, next, true).await
}

/// [`require`], and with `admin_only`, [`require_admin`]. A request it passes refreshes its
/// device's `last_seen_at` at most every [`LAST_SEEN_EVERY`]. When it does, a request
/// authenticated by cookie is answered with the cookie again, unless the handler set one, and
/// with `Cache-Control: private`, unless the handler set its own.
async fn gate(
    state: AppState,
    params: Result<RawPathParams, RawPathParamsRejection>,
    mut request: Request,
    next: Next,
    admin_only: bool,
) -> Response {
    let Some(token) = token(request.headers()) else {
        return unauthenticated();
    };
    let (Token::Bearer(secret) | Token::Cookie(secret)) = token;
    let hash = token_hash(secret);
    let library = library(params);
    let found = match state.db.read(move |conn| device(conn, &hash, library)).await {
        Ok(found) => found,
        Err(error) => return Problem::from(error).into_response(),
    };
    let Some(Found { caller, last_seen_at, reaches_library, revoked }) = found else {
        return unauthenticated();
    };
    if let Some(reason) = revoked {
        return challenge(Problem::revoked(reason));
    }
    // Checked once the token is known to be good, so a bad one is always `401`.
    if matches!(token, Token::Cookie(_))
        && !request.method().is_safe()
        && !request.headers().contains_key(CLIENT)
    {
        return Problem::new(Code::Forbidden)
            .detail("A write authenticated by cookie must send X-Jewelcase-Client.")
            .into_response();
    }
    if admin_only && !caller.admin {
        return Problem::new(Code::Forbidden).into_response();
    }
    if !reaches_library {
        return Problem::not_found().into_response();
    }
    let stale_before = now_ms() - LAST_SEEN_EVERY.as_millis() as i64;
    let stale = last_seen_at < stale_before;
    if stale {
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
    // The cookie is sent again whenever `last_seen_at` is refreshed, so a web session in use
    // never reaches its `Max-Age` (`requirements/users.md` §4).
    let refresh = match token {
        Token::Cookie(secret) if stale => match session::cookie(&state.prefix, secret) {
            Ok(cookie) => Some(cookie),
            Err(problem) => return problem.into_response(),
        },
        _ => None,
    };
    request.extensions_mut().insert(caller);
    if let Some(AnyId(Some(library))) = library {
        request.extensions_mut().insert(Scope { user: caller.user, library });
    }
    let mut response = next.run(request).await;
    if let Some(cookie) = refresh {
        // A cookie the handler set, such as `logout`'s, wins. No shared cache may keep the
        // token, so a handler that sets its own `Cache-Control`, such as an image's, must make
        // it private.
        let headers = response.headers_mut();
        headers.entry(header::SET_COOKIE).or_insert(cookie);
        headers.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("private"));
    }
    response
}

/// A request's token, by where it came from.
enum Token<'a> {
    Bearer(&'a str),
    Cookie(&'a str),
}

/// The request's token. A bearer token wins over the cookie, so a bad one is refused even beside
/// a good cookie. An `Authorization` header with any other scheme, or none, is a proxy's in front
/// of the server (basic auth or SSO), and leaves the cookie to identify the caller.
fn token(headers: &HeaderMap) -> Option<Token<'_>> {
    if let Some(authorization) = headers.get(header::AUTHORIZATION) {
        // Read as bytes: a proxy's credentials need not be text.
        let mut parts = authorization.as_bytes().splitn(2, |byte| matches!(byte, b' ' | b'\t'));
        if parts.next().is_some_and(|scheme| scheme.eq_ignore_ascii_case(b"bearer")) {
            let token = std::str::from_utf8(parts.next().unwrap_or_default()).ok()?;
            return Some(Token::Bearer(token.trim()));
        }
    }
    let cookies = headers.get_all(header::COOKIE).into_iter();
    let mut pairs =
        cookies.filter_map(|value| value.to_str().ok()).flat_map(|value| value.split(';'));
    pairs.find_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('=')).map(Token::Cookie)
}

/// The library the path names, if it names one. An ID that is not one, or a path that cannot
/// be read, names `AnyId(None)`: a library no one reaches.
fn library(params: Result<RawPathParams, RawPathParamsRejection>) -> Option<AnyId> {
    let Ok(params) = params else { return Some(AnyId(None)) };
    let (_, id) = params.iter().find(|(name, _)| *name == "library_id")?;
    Some(AnyId::from(id))
}

/// What [`device`] finds for a token.
struct Found {
    caller: Caller,
    last_seen_at: i64,
    /// Whether the caller reaches the library the path names; true if it names none.
    reaches_library: bool,
    /// Why the device was signed out, if it was.
    revoked: Option<Revoked>,
}

/// The caller behind `hash`, if it is a token of an active account, whether it reaches
/// `library`, which no one does if it does not exist, and whether its device is signed out. A
/// suspended account's tokens answer exactly as unknown ones do, whether or not their devices
/// are signed out (`requirements/users.md` §10).
fn device(
    conn: &Connection,
    hash: &[u8; 32],
    library: Option<AnyId>,
) -> rusqlite::Result<Option<Found>> {
    conn.prepare_cached(concat!(
        "SELECT d.id, d.user_id, d.last_seen_at, u.role <> 'user', \
             EXISTS (SELECT 1 FROM libraries l WHERE l.id = ?2 AND ",
        reaches!(admin: "u.role <> 'user'", user: "u.id", library: "l.id"),
        "), d.revoked_reason \
         FROM devices d JOIN users u ON u.id = d.user_id \
         WHERE d.token_hash = ?1 AND u.status = 'active'"
    ))?
    .query_row(rusqlite::params![hash, library], |row| {
        Ok(Found {
            caller: Caller { device: row.get(0)?, user: row.get(1)?, admin: row.get(3)? },
            last_seen_at: row.get(2)?,
            reaches_library: library.is_none() || row.get(4)?,
            revoked: row.get::<_, Option<Text<_>>>(5)?.map(|Text(reason)| reason),
        })
    })
    .optional()
}

/// A `401 unauthenticated`.
fn unauthenticated() -> Response {
    challenge(Problem::new(Code::Unauthenticated))
}

/// `problem`, which must be a `401`, with the challenge RFC 9110 asks every `401` to name.
fn challenge(problem: Problem) -> Response {
    let challenge = [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))];
    (challenge, problem).into_response()
}

#[cfg(test)]
mod tests;
