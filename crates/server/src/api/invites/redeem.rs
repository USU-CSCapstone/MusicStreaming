//! The public half of invites: `checkInvite` and `redeemInvite` (`requirements/users.md` §2.2).
//! Anyone may call them, so a code that is not live is refused before anything else in the
//! request is looked at, and every such code (unknown, redeemed, revoked or expired) answers the
//! same `404`.

#[cfg(test)]
mod tests;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use serde::Deserialize;

use crate::api::audit::{self, Action};
use crate::api::devices::DeviceRegistration;
use crate::api::extract;
use crate::api::login::limits;
use crate::api::origin::Origin;
use crate::api::session::{self, token_hash};
use crate::api::{AppState, Code, Problem, me, password};
use crate::db::{liked_songs, new_id, now_ms};

/// Which invite is live: the one whose code hashes to `?1`, not redeemed or revoked, and not
/// expired at `?2`, whether or not a sweep has run since.
const LIVE: &str =
    "code_hash = ?1 AND redeemed_at IS NULL AND revoked_at IS NULL AND expires_at > ?2";

/// The body of `checkInvite`.
#[derive(Deserialize)]
pub struct CheckRequest {
    code: String,
}

/// Whether the invite whose code hashes to `hash` is live at `now`.
fn live(conn: &Connection, hash: &[u8; 32], now: i64) -> rusqlite::Result<bool> {
    conn.prepare_cached(&format!("SELECT 1 FROM invites WHERE {LIVE}"))?.exists(params![hash, now])
}

/// Refuses with `404` unless the invite whose code hashes to `hash` is live.
async fn require_live(state: &AppState, hash: [u8; 32]) -> Result<(), Problem> {
    if state.db.read(move |conn| live(conn, &hash, now_ms())).await? {
        Ok(())
    } else {
        Err(Problem::not_found())
    }
}

/// `checkInvite`: `204` if the code is live, else `404`.
pub async fn check(
    State(state): State<AppState>,
    extract::Json(request): extract::Json<CheckRequest>,
) -> Result<StatusCode, Problem> {
    require_live(&state, token_hash(&request.code)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The spec's `RedeemInviteRequest`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedeemRequest {
    code: String,
    username: String,
    password: String,
    display_name: Option<String>,
    device: DeviceRegistration,
}

/// `redeemInvite`: makes a `user` account with the invite's libraries and signs it in on this
/// device, answering as `completeSetup` does. The invite is consumed in the write that makes the
/// account, so one code makes one account however many redeem it at once. A taken username is
/// `409 username_taken`, and leaves the invite pending.
pub async fn redeem(
    State(state): State<AppState>,
    from: Origin,
    headers: HeaderMap,
    extract::Json(request): extract::Json<RedeemRequest>,
) -> Result<Response, Problem> {
    let RedeemRequest { code, username, password, display_name, device } = request;
    let code_hash = token_hash(&code);
    // Before any hashing, and before the username is looked at.
    require_live(&state, code_hash).await?;
    me::check_username(&username)?;
    let display_name = display_name.unwrap_or_else(|| username.clone());
    me::check_name("displayName", &display_name)?;
    device.validate()?;
    let password = password::hash_new(password, vec![username.clone(), display_name.clone()]);
    let password = password.await?;
    let token = session::new_token()?;
    let hash = token_hash(&token);

    let session = state
        .db
        .try_write(move |tx| {
            let now = now_ms();
            // Checked again where writes are serialized: the invite may have been used, revoked
            // or expired since.
            let consumed = tx
                .prepare_cached(&format!(
                    "UPDATE invites SET redeemed_at = ?2 WHERE {LIVE} RETURNING id, created_by"
                ))?
                .query_row(params![code_hash, now], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let Some((invite, inviter)): Option<(i64, Option<i64>)> = consumed else {
                return Err(Problem::not_found());
            };
            let user = new_id()?;
            let created = tx.execute(
                "INSERT INTO users (id, username, display_name, role, password, created_at) \
                 VALUES (?1, ?2, ?3, 'user', ?4, ?5)",
                params![user, username, display_name, password, now],
            );
            match created {
                Err(rusqlite::Error::SqliteFailure(error, _))
                    if error.code == ErrorCode::ConstraintViolation
                        && error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
                {
                    return Err(Problem::new(Code::UsernameTaken));
                }
                created => created?,
            };
            // Set once the account exists, for the foreign key.
            tx.execute("UPDATE invites SET redeemed_by = ?2 WHERE id = ?1", [invite, user])?;
            let mut library_ids = tx
                .prepare_cached(
                    "INSERT INTO library_access (user_id, library_id, granted_at) \
                     SELECT ?1, library_id, ?2 FROM invite_libraries WHERE invite_id = ?3 \
                     RETURNING library_id",
                )?
                .query_map([user, now, invite], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()?;
            library_ids.sort_unstable();
            liked_songs::ensure(tx)?;
            let attempt = limits::record(tx, &username, Some(user), from.client, now)?;
            limits::succeeded(tx, attempt)?;
            let session = session::register(tx, user, &device, &hash)?;
            audit::record(
                tx,
                Action::AccountCreated { invite_id: invite, library_ids },
                inviter,
                user,
            )?;
            Ok(session)
        })
        .await?;

    Ok(session.respond(token, StatusCode::CREATED, &state.prefix, &from, &headers))
}
