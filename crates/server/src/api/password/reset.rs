//! Password reset links (`requirements/users.md` §3): an admin issues one for an account with
//! `adminIssuePasswordReset`, and whoever holds it sets the account's password with
//! `checkPasswordReset` and `redeemPasswordReset`. The admin never learns or sets the password.
//! An account has at most one live link, kept as the hash of its code with when it expires.
//! Anyone may call the two public operations, so a code that is not live is refused before
//! anything else is done, and every such code answers the same `404`.

#[cfg(test)]
mod tests;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use super::{confirm, hash_new};
use crate::api::accounts::authorize;
use crate::api::audit::{self, Action};
use crate::api::devices::{self, Which};
use crate::api::extract::{self, Path};
use crate::api::origin::Origin;
use crate::api::playlists::settle;
use crate::api::realtime::Revoked;
use crate::api::session::{new_token, token_hash};
use crate::api::time::serialize_rfc3339;
use crate::api::{AppState, Caller, Id, Problem};
use crate::db::{Database, now_ms};

/// How long a link lasts once issued: a day, as NIST SP 800-63B allows at most.
const LIFETIME_MS: i64 = 86_400_000;

/// Which account's link is live: the one whose code hashes to `?1`, not expired at `?2`, of an
/// account that is active, so a suspended account's link cannot be used.
const LIVE: &str = "reset_hash = ?1 AND reset_expires_at > ?2 AND status = 'active'";

/// The spec's `IssuePasswordResetRequest`: the admin's own current password.
#[derive(Deserialize)]
pub struct IssueRequest {
    password: String,
}

/// The spec's `IssuedPasswordReset`: the link's code, shown this once, and when it expires.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issued {
    code: String,
    #[serde(serialize_with = "serialize_rfc3339")]
    expires_at: i64,
}

/// `adminIssuePasswordReset`: once the caller's password is confirmed, gives account `userId`
/// a link that lasts [`LIFETIME_MS`], replacing any earlier one, and answers its code. Nothing
/// else changes: the account's password and sessions keep working until the link is used. The
/// caller and their rank over the account are checked again in the write, so authority lost
/// since the request began issues nothing.
pub async fn issue(
    State(state): State<AppState>,
    caller: Caller,
    from: Origin,
    Path(Id(target)): Path<Id>,
    extract::Json(IssueRequest { password }): extract::Json<IssueRequest>,
) -> Result<(StatusCode, Json<Issued>), Problem> {
    let actor = caller.user;
    let confirmed = confirm(&state, caller, &from, password).await?;
    let code = new_token()?;
    let hash = token_hash(&code);
    let expires_at = state
        .db
        .try_write(move |tx| {
            confirmed.check(tx)?;
            authorize(tx, actor, target)?;
            Ok::<_, Problem>(set(tx, Some(actor), target, hash)?)
        })
        .await?;
    Ok((StatusCode::CREATED, Json(Issued { code, expires_at })))
}

/// Gives account `target` a link that lasts [`LIFETIME_MS`] from now, whose code hashes to
/// `hash`, replacing any earlier one, and records that `actor` (none for the host) issued it.
/// Answers when it expires.
fn set(tx: &Transaction, actor: Option<i64>, target: i64, hash: [u8; 32]) -> rusqlite::Result<i64> {
    let expires_at = now_ms() + LIFETIME_MS;
    tx.prepare_cached("UPDATE users SET reset_hash = ?2, reset_expires_at = ?3 WHERE id = ?1")?
        .execute(params![target, hash, expires_at])?;
    audit::record(tx, Action::PasswordResetIssued, actor, target)?;
    Ok(expires_at)
}

/// Gives the owner a link as `adminIssuePasswordReset` would, done by no one, for an owner who
/// cannot sign in and whose host asked for it. Answers its code, or `None` before setup, when
/// there is no owner. Nothing else about the owner changes.
pub(crate) async fn issue_for_owner(db: &Database) -> anyhow::Result<Option<String>> {
    let code = new_token().map_err(|_| anyhow::anyhow!("cannot make a reset code"))?;
    let hash = token_hash(&code);
    let owner = db
        .write(move |tx| {
            let owner = tx
                .query_row("SELECT id FROM users WHERE role = 'owner'", [], |row| row.get(0))
                .optional()?;
            owner.map(|owner| set(tx, None, owner, hash)).transpose()
        })
        .await?;
    Ok(owner.map(|_| code))
}

/// The spec's `CheckPasswordResetRequest`.
#[derive(Deserialize)]
pub struct CheckRequest {
    code: String,
}

/// The spec's `PasswordResetAccount`: whose password the link sets.
#[derive(Serialize)]
pub struct Account {
    username: String,
}

/// The username and display name of the account whose link is live with the code that hashes
/// to `hash`, else `404`.
async fn live(state: &AppState, hash: [u8; 32]) -> Result<(String, String), Problem> {
    let account = state
        .db
        .read(move |conn| {
            conn.prepare_cached(&format!("SELECT username, display_name FROM users WHERE {LIVE}"))?
                .query_row(params![hash, now_ms()], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()
        })
        .await?;
    account.ok_or_else(Problem::not_found)
}

/// `checkPasswordReset`: the username of the account whose link is live with the code, else
/// `404`.
pub async fn check(
    State(state): State<AppState>,
    extract::Json(CheckRequest { code }): extract::Json<CheckRequest>,
) -> Result<Json<Account>, Problem> {
    let (username, _) = live(&state, token_hash(&code)).await?;
    Ok(Json(Account { username }))
}

/// The spec's `RedeemPasswordResetRequest`.
#[derive(Deserialize)]
pub struct RedeemRequest {
    code: String,
    password: String,
}

/// `redeemPasswordReset`: sets the account's password, uses up the link, and signs out every
/// device of the account with `credentials_changed`, closing their connections. It does not
/// sign anyone in. The link is consumed by the write that sets the password, guarded on it still
/// being live, so a link used, replaced or cleared, or an account suspended, since the request
/// began changes nothing, and one code sets one password however many redeem it at once.
pub async fn redeem(
    State(state): State<AppState>,
    extract::Json(RedeemRequest { code, password }): extract::Json<RedeemRequest>,
) -> Result<StatusCode, Problem> {
    let code_hash = token_hash(&code);
    // Before any hashing.
    let (username, display_name) = live(&state, code_hash).await?;
    let password = hash_new(password, vec![username, display_name]).await?;

    let (db, hub) = (state.db.clone(), state.hub.clone());
    // Settled: a client that hangs up drops this request, which must not leave the account's
    // connections open once the reset is committed.
    settle(async move {
        let user = db
            .try_write(move |tx| {
                let consumed = tx
                    .prepare_cached(&format!(
                        "UPDATE users SET password = ?3, reset_hash = NULL, \
                         reset_expires_at = NULL WHERE {LIVE} RETURNING id"
                    ))?
                    .query_row(params![code_hash, now_ms(), password], |row| row.get(0))
                    .optional()?;
                let Some(user) = consumed else {
                    return Err(Problem::not_found());
                };
                devices::sign_out(
                    tx,
                    user,
                    Which::All { keep: None },
                    Revoked::CredentialsChanged,
                )?;
                audit::record(tx, Action::PasswordResetUsed, None, user)?;
                Ok(user)
            })
            .await?;
        hub.revoke_all(user, Revoked::CredentialsChanged);
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
