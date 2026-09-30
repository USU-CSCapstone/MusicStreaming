//! `login`: username and password in, a new device's session out (`requirements/users.md` §3).
//!
//! Every refusal is the same `401 invalid_credentials`, whether the account is unknown, the
//! password wrong, or the account suspended, and each takes one password check's time. Past
//! the limits ([`limits`]) an attempt is `429 rate_limited` without its password being checked.

mod limits;

use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;

use super::extract::Json;
use super::origin::Origin;
use super::session::{self, DeviceRegistration};
use super::{AppState, Code, Problem, password};
use crate::db::now_ms;

#[derive(Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
    device: DeviceRegistration,
    // `deviceId` reclaims a device logged out by a password change, which nothing can do yet,
    // so every login registers a new device, as the spec says for an ID that is not one.
}

pub async fn login(
    State(state): State<AppState>,
    Origin(origin): Origin,
    Json(request): Json<LoginRequest>,
) -> Result<Response, Problem> {
    let LoginRequest { username, password, device } = request;
    // No account can have another name, and this keeps megabyte names out of the records.
    session::check_username(&username)?;
    device.validate()?;

    let origin = origin.to_string();
    let attempt = state
        .db
        .write(move |tx| {
            let now = now_ms();
            let wait = limits::wait_ms(tx, &username, &origin, now)?;
            if wait > 0 {
                // Not recorded: a write per refusal would let anyone keep the disk busy.
                return Ok(Err(wait));
            }
            let account = account(tx, &username)?;
            let user = account.as_ref().map(|(id, _, _)| *id);
            let id = limits::record(tx, &username, user, &origin, now)?;
            Ok(Ok((id, account)))
        })
        .await?;
    let (attempt, account) = match attempt {
        Ok(attempt) => attempt,
        Err(wait_ms) => return Ok(rate_limited(wait_ms)),
    };

    let (user, stored, active) = match account {
        Some((user, stored, active)) => (Some(user), Some(stored), active),
        None => (None, None, false),
    };
    let matches = password::verify(password, stored).await?;
    let Some(user) = user.filter(|_| matches && active) else {
        return Err(Problem::new(Code::InvalidCredentials));
    };

    let token = session::new_token()?;
    let session = state
        .db
        .write(move |tx| {
            limits::succeeded(tx, attempt)?;
            session::register(tx, user, &device, token)
        })
        .await?;
    let cookie = session::cookie(&state.prefix, &session.token);
    Ok((StatusCode::OK, [(header::SET_COOKIE, cookie)], axum::Json(session)).into_response())
}

/// The account `username` names: its ID, password, and whether it may sign in.
fn account(conn: &Connection, username: &str) -> rusqlite::Result<Option<(i64, String, bool)>> {
    conn.prepare_cached("SELECT id, password, status = 'active' FROM users WHERE username = ?1")?
        .query_row([username], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()
}

/// A `429` that says, in whole seconds rounded up, when to try again.
fn rate_limited(wait_ms: i64) -> Response {
    let seconds = HeaderValue::from((wait_ms + 999) / 1000);
    ([(header::RETRY_AFTER, seconds)], Problem::new(Code::RateLimited)).into_response()
}

#[cfg(test)]
mod tests;
