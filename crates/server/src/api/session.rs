//! Sessions: the token every login is given, the `Session` that setup and login answer with,
//! and `logout` (`requirements/users.md` §4).
//!
//! A token is shown once and stored only as its SHA-256 hash (`design/database.md`). It is
//! 256 random bits, so a fast hash is enough; a slow one would only slow every request.

use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use rusqlite::Transaction;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::devices::{self, Device, DeviceRegistration};
use super::me::{self, User};
use super::realtime::Revoked;
use super::scope::Caller;
use super::{AppState, Code, Problem};
use crate::db::DbError;

/// The cookie browser clients authenticate with (`cookieAuth` in `api/openapi.yaml`).
pub const COOKIE: &str = "jewelcase_session";

/// Browsers keep a cookie for at most 400 days, so that is how long it asks for. A session in
/// use is sent it again (`authenticate`), so only one unused that long expires.
const COOKIE_MAX_AGE_S: u32 = 400 * 24 * 60 * 60;

#[derive(Serialize)]
pub struct Session {
    pub token: String,
    user: User,
    device: Device,
}

/// A new, random token.
pub fn new_token() -> Result<String, Problem> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|error| {
        tracing::error!(%error, "cannot read the system's random source");
        Problem::new(Code::Internal)
    })?;
    Ok(BASE64URL.encode(bytes))
}

/// What the database stores in place of `token`.
pub fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Registers `device` for `user`, signed in with `token`, and returns its session.
pub fn register(
    tx: &Transaction,
    user: i64,
    device: &DeviceRegistration,
    token: String,
) -> rusqlite::Result<Session> {
    let device = devices::register(tx, user, device, &token_hash(&token))?;
    Ok(Session { token, user: me::user(tx, user)?, device })
}

/// `logout`: removes the calling device, closes its realtime connections, and tells a browser
/// to forget its cookie.
pub async fn logout(State(state): State<AppState>, caller: Caller) -> Result<Response, Problem> {
    let (db, hub) = (state.db.clone(), state.hub.clone());
    // Spawned: a client that hangs up drops this request, which must not leave the device's
    // connections open once its row is gone.
    let logout = tokio::spawn(async move {
        db.write(move |tx| tx.execute("DELETE FROM devices WHERE id = ?1", [caller.device]))
            .await?;
        hub.revoke(caller.user, caller.device, Revoked::LoggedOut);
        devices::changed(db, hub, caller.user);
        Ok::<_, DbError>(())
    });
    logout.await.map_err(|error| {
        tracing::error!(%error, "logging out did not complete");
        Problem::new(Code::Internal)
    })??;
    let forget = cookie_header(format!("{COOKIE}=; Path={}; Max-Age=0", state.prefix));
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, forget)]).into_response())
}

/// The `Set-Cookie` value that has a browser send `token` with every request to the API at
/// `prefix`, and nowhere else. Scripts cannot read it, and other sites cannot send it.
pub fn cookie(prefix: &str, token: &str) -> HeaderValue {
    cookie_header(format!(
        "{COOKIE}={token}; Path={prefix}; Max-Age={COOKIE_MAX_AGE_S}; HttpOnly; SameSite=Strict"
    ))
}

fn cookie_header(cookie: String) -> HeaderValue {
    // Tokens are base64url and the base path is limited to URL-safe characters (`config`).
    HeaderValue::try_from(cookie).expect("a cookie of header-safe characters")
}
