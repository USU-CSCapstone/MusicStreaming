//! Sessions: the token every login is given, the `Session` that setup and login answer with,
//! and `logout` (`requirements/users.md` §4).
//!
//! A token is shown once and stored only as its SHA-256 hash (`design/database.md`). It is
//! 256 random bits, so a fast hash is enough; a slow one would only slow every request. The
//! web client is never shown it: the cookie carries its session, out of reach of script.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use rusqlite::Transaction;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::authenticate::CLIENT;
use super::devices::{self, Device, DeviceRegistration};
use super::me::{self, User};
use super::origin::Origin;
use super::realtime::Revoked;
use super::scope::Caller;
use super::{AppState, Code, Problem};
use crate::db::DbError;

/// The cookie browser clients authenticate with (`cookieAuth` in `api/openapi.yaml`).
pub const COOKIE: &str = "jewelcase_session";

/// Browsers keep a cookie for at most 400 days, so that is how long it asks for. A session in
/// use is sent it again (`authenticate`), so only one unused that long expires.
const COOKIE_MAX_AGE_S: u32 = 400 * 24 * 60 * 60;

/// The spec's `Session`: a new device's account, the device, and, for any client but the web
/// client, its token.
#[derive(Serialize)]
pub struct Session {
    /// Set by [`Session::respond`]; left out of the JSON when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    user: User,
    device: Device,
}

/// SQL that is true where the device `d` is still signed in and its account `u`, joined on
/// `u.id = d.user_id`, may still sign in.
macro_rules! device_signed_in {
    () => {
        "d.revoked_reason IS NULL AND u.status = 'active'"
    };
}
pub(super) use device_signed_in;

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

/// Registers `device` for `user`, signed in with the token whose [`token_hash`] is `hash`, and
/// returns its session without the token, which [`Session::respond`] adds.
pub fn register(
    tx: &Transaction,
    user: i64,
    device: &DeviceRegistration,
    hash: &[u8; 32],
) -> rusqlite::Result<Session> {
    let device = devices::register(tx, user, device, hash)?;
    Ok(Session { token: None, user: me::user(tx, user)?, device })
}

impl Session {
    /// The `status` response to a sign-in with `token`: this session as JSON, and the [`cookie`]
    /// for `prefix` and `from`. The body carries `token` too unless the request's `headers` hold
    /// `X-Jewelcase-Client`, as the web client's do, so no script on its page sees the token.
    /// Fails as [`cookie`] does.
    pub fn respond(
        mut self,
        token: String,
        status: StatusCode,
        prefix: &str,
        from: &Origin,
        headers: &HeaderMap,
    ) -> Result<Response, Problem> {
        let cookie = cookie(prefix, &token, from)?;
        if !headers.contains_key(CLIENT) {
            self.token = Some(token);
        }
        Ok((status, [(header::SET_COOKIE, cookie)], Json(self)).into_response())
    }
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
    let forget = cookie_header(format!("{COOKIE}=; Path={}; Max-Age=0", state.prefix))?;
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, forget)]).into_response())
}

/// The `Set-Cookie` value that has a browser send `token` with every request to the API at
/// `prefix`, and nowhere else. Scripts cannot read it, and other sites cannot send it. A client
/// that came over https, as `from` says, sends it only over https. Fails, as `cookie_header`
/// does, only if `prefix` or `token` holds a character a header can't.
pub fn cookie(prefix: &str, token: &str, from: &Origin) -> Result<HeaderValue, Problem> {
    let secure = if from.https { "; Secure" } else { "" };
    cookie_header(format!(
        "{COOKIE}={token}; Path={prefix}; Max-Age={COOKIE_MAX_AGE_S}; HttpOnly; SameSite=Strict{secure}"
    ))
}

/// `cookie` as a `Set-Cookie` value. Tokens are base64url and the base path is limited to
/// URL-safe characters (`config`), so it fails only if one of them is not.
fn cookie_header(cookie: String) -> Result<HeaderValue, Problem> {
    HeaderValue::try_from(cookie).map_err(|error| {
        tracing::error!(%error, "cannot make a cookie");
        Problem::new(Code::Internal)
    })
}
