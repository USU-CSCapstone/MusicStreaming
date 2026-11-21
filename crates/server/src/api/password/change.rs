//! `changeMyPassword`: a signed-in person changes their own password with their current one,
//! which signs out every other device of theirs (`requirements/users.md` §3).

#[cfg(test)]
mod tests;

use axum::extract::State;
use axum::http::StatusCode;
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;

use super::{confirm, hash_new};
use crate::api::devices::{self, Which, still_signed_in};
use crate::api::extract;
use crate::api::origin::Origin;
use crate::api::playlists::settle;
use crate::api::realtime::Revoked;
use crate::api::{AppState, Caller, Code, Problem};

/// The spec's `ChangePasswordRequest`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

/// `changeMyPassword`: once the current password is confirmed and the new one is strong
/// enough, sets it and clears any reset link, and signs out the caller's other devices with
/// `credentials_changed`, closing their connections and sending the rest the new list. The
/// calling device stays signed in with the same token.
pub async fn change(
    State(state): State<AppState>,
    caller: Caller,
    from: Origin,
    extract::Json(request): extract::Json<ChangePasswordRequest>,
) -> Result<StatusCode, Problem> {
    let ChangePasswordRequest { current_password, new_password } = request;
    let Caller { user, device, .. } = caller;
    // What the new password must not be built from.
    let inputs = state
        .db
        .read(move |conn| {
            conn.prepare_cached(
                "SELECT username, display_name FROM users WHERE id = ?1 AND status = 'active'",
            )?
            .query_row([user], |row| Ok(vec![row.get(0)?, row.get(1)?]))
            .optional()
        })
        .await?
        .ok_or_else(|| Problem::new(Code::Unauthenticated))?;
    let confirmed = confirm(&state, caller, &from, current_password).await?;
    let hash = hash_new(new_password, inputs).await?;

    let (db, hub) = (state.db.clone(), state.hub.clone());
    // Settled: a client that hangs up drops this request, which must not leave the other
    // devices' connections open once the change is committed.
    settle(async move {
        let signed_out = db
            .try_write(move |tx| {
                confirmed.check(tx)?;
                still_signed_in(tx, user, device)?;
                tx.prepare_cached(
                    "UPDATE users SET password = ?2, reset_hash = NULL, reset_expires_at = NULL \
                     WHERE id = ?1",
                )?
                .execute(params![user, hash])?;
                let others = Which::All { keep: Some(device) };
                Ok::<_, Problem>(devices::sign_out(tx, user, others, Revoked::CredentialsChanged)?)
            })
            .await?;
        if signed_out.is_empty() {
            return Ok(());
        }
        for (other, _) in signed_out {
            hub.revoke(user, other, Revoked::CredentialsChanged);
        }
        devices::changed(db, hub, user);
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
