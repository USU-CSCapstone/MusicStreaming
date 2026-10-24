//! First-run setup: `getServerInfo` and `completeSetup` (`requirements/users.md` §2.1).
//!
//! A new server has no accounts and no default credentials. Until setup creates the owner, it
//! serves nothing but the setup flow: every other endpoint answers `503 setup_required`.

use std::sync::atomic::{AtomicBool, Ordering};

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use super::devices::DeviceRegistration;
use super::extract;
use super::login::limits;
use super::origin::Origin;
use super::{AppState, Code, Problem, me, password, session};
use crate::db::{Database, new_id, now_ms};

/// Whether setup is done, which is whether the owner exists.
#[derive(Default)]
pub struct Setup(AtomicBool);

impl Setup {
    /// Whether the owner exists. The owner can never be deleted, so once one does this stops
    /// reading the database.
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

/// Whether the owner's account exists.
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

/// The spec's `ServerInfo`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    version: &'static str,
    api_version: &'static str,
    setup_required: bool,
}

/// `getServerInfo`: the server's version, the API's, and whether setup is still to be done.
pub async fn server_info(State(state): State<AppState>) -> Result<Json<ServerInfo>, Problem> {
    Ok(Json(ServerInfo {
        version: env!("CARGO_PKG_VERSION"),
        // The version in the API's path.
        api_version: "1",
        setup_required: !state.setup.done(&state.db).await?,
    }))
}

/// The spec's `SetupRequest`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRequest {
    username: String,
    password: String,
    display_name: Option<String>,
    device: DeviceRegistration,
}

/// Creates the owner and logs it in on this device, answering as [`session::Session::respond`]
/// does. It counts as a sign-in that succeeded from the client `from` names (`login::limits`).
/// Once an owner exists this is `404`, as if the endpoint were not there.
pub async fn complete(
    State(state): State<AppState>,
    from: Origin,
    headers: HeaderMap,
    extract::Json(request): extract::Json<SetupRequest>,
) -> Result<Response, Problem> {
    if state.setup.done(&state.db).await? {
        return Err(Problem::not_found());
    }
    let SetupRequest { username, password, display_name, device } = request;
    me::check_username(&username)?;
    let display_name = display_name.unwrap_or_else(|| username.clone());
    me::check_name("displayName", &display_name)?;
    device.validate()?;
    let password = password::hash_new(password, vec![username.clone(), display_name.clone()]);
    let password = password.await?;
    let token = session::new_token()?;
    let hash = session::token_hash(&token);

    let session = state
        .db
        .try_write(move |tx| {
            // Checked again where writes are serialized, so two setups at once create one owner.
            if has_owner(tx)? {
                return Err(Problem::not_found());
            }
            let now = now_ms();
            let user = new_id()?;
            tx.execute(
                "INSERT INTO users (id, username, display_name, role, password, created_at) \
                 VALUES (?1, ?2, ?3, 'owner', ?4, ?5)",
                params![user, username, display_name, password, now],
            )?;
            let attempt = limits::record(tx, &username, Some(user), from.client, now)?;
            limits::succeeded(tx, attempt)?;
            Ok(session::register(tx, user, &device, &hash)?)
        })
        .await?;
    state.setup.0.store(true, Ordering::Relaxed);

    Ok(session.respond(token, StatusCode::CREATED, &state.prefix, &from, &headers))
}

#[cfg(test)]
mod tests;
