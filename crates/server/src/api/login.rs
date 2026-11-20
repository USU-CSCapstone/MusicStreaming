//! `login`: username and password in, a new device's session out (`requirements/users.md` §3).
//!
//! Every refusal is the same `401 invalid_credentials`, whether the account is unknown, the
//! password wrong, or the account suspended, and each takes one password check's time. Past
//! the limits ([`limits`]) an attempt is `429 rate_limited` without its password being checked.

pub(super) mod limits;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;

use super::devices::{self, DeviceRegistration};
use super::extract;
use super::origin::Origin;
use super::{AppState, Code, Problem, me, password, session};
use crate::db::now_ms;

/// The spec's `LoginRequest`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    username: String,
    password: String,
    device: DeviceRegistration,
    /// The ID of a device a password change signed out, to be signed in again as itself.
    /// Anything that is not one is ignored.
    device_id: Option<String>,
}

/// Logs `username` in on a new device, or on the one `deviceId` names if a password change
/// signed it out, answering as [`session::Session::respond`] does, or
/// refuses as the module docs describe.
pub async fn login(
    State(state): State<AppState>,
    from: Origin,
    headers: HeaderMap,
    extract::Json(request): extract::Json<LoginRequest>,
) -> Result<Response, Problem> {
    let LoginRequest { username, password, device, device_id } = request;
    let reclaim = device_id.and_then(|id| id.parse::<i64>().ok());
    // No account can have another name, and this keeps megabyte names out of the records.
    me::check_username(&username)?;
    device.validate()?;

    let (attempt, account) = state
        .db
        .try_write(move |tx| {
            let now = now_ms();
            let wait = limits::wait_ms(tx, &username, from.client, now)?;
            if wait > 0 {
                // Not recorded: a write per refusal would let anyone keep the disk busy.
                return Err(Problem::rate_limited(wait));
            }
            let account = account(tx, &username)?;
            let user = account.as_ref().map(|(id, _, _)| *id);
            let id = limits::record(tx, &username, user, from.client, now)?;
            Ok((id, account))
        })
        .await?;

    let (user, stored, active) = match account {
        Some((user, stored, active)) => (Some(user), Some(stored), active),
        None => (None, None, false),
    };
    let matches = password::verify(password, stored.clone()).await?;
    let (Some(user), Some(stored)) = (user.filter(|_| matches && active), stored) else {
        return Err(Problem::new(Code::InvalidCredentials));
    };

    let token = session::new_token()?;
    let hash = session::token_hash(&token);
    let session = state
        .db
        .try_write(move |tx| {
            // A suspension or a password change committed while the password was checked has
            // signed out every device it found, so this one must not be added after it.
            if !may_sign_in(tx, user, &stored)? {
                return Err(Problem::new(Code::InvalidCredentials));
            }
            limits::succeeded(tx, attempt)?;
            Ok(session::register_or_reclaim(tx, user, &device, &hash, reclaim)?)
        })
        .await?;
    devices::changed(state.db.clone(), state.hub.clone(), user);
    Ok(session.respond(token, StatusCode::OK, &state.prefix, &from, &headers))
}

/// The account `username` names: its ID, password, and whether it may sign in.
fn account(conn: &Connection, username: &str) -> rusqlite::Result<Option<(i64, String, bool)>> {
    conn.prepare_cached("SELECT id, password, status = 'active' FROM users WHERE username = ?1")?
        .query_row([username], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()
}

/// Whether account `user` still exists, is active, and has the password `stored` that was checked.
fn may_sign_in(conn: &Connection, user: i64, stored: &str) -> rusqlite::Result<bool> {
    conn.prepare_cached("SELECT status = 'active' AND password = ?2 FROM users WHERE id = ?1")?
        .query_row(rusqlite::params![user, stored], |row| row.get(0))
        .optional()
        .map(|active| active.unwrap_or(false))
}

#[cfg(test)]
mod tests;
