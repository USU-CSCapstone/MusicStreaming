//! First-run setup: `getServerInfo` and `completeSetup` (`requirements/users.md` §2.1).
//!
//! A new server has no accounts and no default credentials. Until setup creates the owner, it
//! serves nothing but the setup flow: every other endpoint answers `503 setup_required`.

use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use super::extract::Json;
use super::session::{self, DeviceRegistration};
use super::{AppState, Code, Problem, password};
use crate::db::{Database, now_ms};

/// Whether setup is done, which is whether the owner exists.
#[derive(Default)]
pub struct Setup(AtomicBool);

impl Setup {
    /// The owner can never be deleted, so once one exists this stops reading the database.
    async fn done(&self, db: &Database) -> Result<bool, Problem> {
        if self.0.load(Ordering::Relaxed) {
            return Ok(true);
        }
        let done = db.read(has_owner).await?;
        if done {
            self.0.store(true, Ordering::Relaxed);
        }
        Ok(done)
    }
}

fn has_owner(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM users WHERE role = 'owner')", [], |row| row.get(0))
}

/// Answers `503 setup_required` in place of the routes it wraps until setup is done.
pub async fn require(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, Problem> {
    if !state.setup.done(&state.db).await? {
        return Err(Problem::new(Code::SetupRequired));
    }
    Ok(next.run(request).await)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    version: &'static str,
    api_version: &'static str,
    setup_required: bool,
}

pub async fn server_info(State(state): State<AppState>) -> Result<axum::Json<ServerInfo>, Problem> {
    Ok(axum::Json(ServerInfo {
        version: env!("CARGO_PKG_VERSION"),
        // The version in the API's path.
        api_version: "1",
        setup_required: !state.setup.done(&state.db).await?,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRequest {
    username: String,
    password: String,
    display_name: Option<String>,
    device: DeviceRegistration,
}

/// Creates the owner and logs it in on this device, setting the session cookie. Once an owner
/// exists this is `404`, as if the endpoint were not there.
pub async fn complete(
    State(state): State<AppState>,
    Json(request): Json<SetupRequest>,
) -> Result<Response, Problem> {
    if state.setup.done(&state.db).await? {
        return Err(Problem::not_found());
    }
    let SetupRequest { username, password, display_name, device } = request;
    check_username(&username)?;
    let display_name = display_name.unwrap_or_else(|| username.clone());
    session::check_name("displayName", &display_name)?;
    device.validate()?;
    let password = password::hash_new(password, vec![username.clone(), display_name.clone()]);
    let password = password.await?;
    let token = session::new_token()?;

    let session = state
        .db
        .write(move |tx| {
            // Checked again where writes are serialized, so two setups at once create one owner.
            if has_owner(tx)? {
                return Ok(None);
            }
            let now = now_ms();
            let user = tx.query_row(
                "INSERT INTO users (id, username, display_name, role, password, created_at, \
                 updated_at) VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, 'owner', ?3, ?4, ?4) \
                 RETURNING id",
                params![username, display_name, password, now],
                |row| row.get(0),
            )?;
            session::register(tx, user, &device, token).map(Some)
        })
        .await?
        .ok_or_else(Problem::not_found)?;
    state.setup.0.store(true, Ordering::Relaxed);

    let cookie = session::cookie(&state.prefix, &session.token);
    Ok((StatusCode::CREATED, [(header::SET_COOKIE, cookie)], axum::Json(session)).into_response())
}

/// The spec's `Username`: 1 to 32 ASCII letters, digits, `_`, and `-`.
fn check_username(username: &str) -> Result<(), Problem> {
    let allowed = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    if (1..=32).contains(&username.len()) && username.bytes().all(allowed) {
        Ok(())
    } else {
        Err(Problem::invalid("username must be 1 to 32 letters, digits, '_', or '-'"))
    }
}

#[cfg(test)]
mod tests;
