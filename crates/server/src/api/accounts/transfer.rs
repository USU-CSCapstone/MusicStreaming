//! `transferOwnership`: the owner hands the server to an active admin and becomes an admin
//! themselves (`requirements/users.md` §1, §8).

#[cfg(test)]
mod tests;

use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;

use super::authorize::OWNER;
use super::{authorize, manager_rank};
use crate::api::audit::{self, Action};
use crate::api::extract;
use crate::api::origin::Origin;
use crate::api::password::confirm;
use crate::api::playlists::settle;
use crate::api::realtime::ServerMessage;
use crate::api::{AppState, Caller, Code, Id, Problem};

/// The spec's `OwnershipTransfer`: the admin to make the owner and the owner's own current
/// password.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    user_id: Id,
    password: String,
}

/// `transferOwnership`: once the caller's password is confirmed, makes the caller an admin and
/// account `userId` the owner, in one write that checks again that the caller is the owner,
/// with that password, and that the account is an active admin they may act on. The owner is
/// demoted before the admin is promoted, since the index that allows one owner refuses a swap
/// in one statement; each statement must change its one row, or the write rolls back whole, so
/// there is never no owner or two. The new owner's reset link is cleared, and so is every
/// admin's, the former owner's included: only the owner can issue a link for an admin, so each
/// came from the account that is no longer owner, and would otherwise reach past the rank rule.
/// A user's link stays, since an admin may still reset a user. Recorded, and both accounts'
/// devices are told once it is committed.
pub async fn transfer(
    State(state): State<AppState>,
    caller: Caller,
    from: Origin,
    extract::Json(Transfer { user_id: Id(target), password }): extract::Json<Transfer>,
) -> Result<StatusCode, Problem> {
    let actor = caller.user;
    let confirmed = confirm(&state, caller, &from, password).await?;
    let AppState { db, hub, .. } = state;
    // Settled: an owner who hangs up drops this request, which must not leave either account's
    // devices untold once the transfer is committed.
    settle(async move {
        db.try_write(move |tx| {
            confirmed.check(tx)?;
            if manager_rank(tx, actor)? != OWNER {
                return Err(
                    Problem::new(Code::Forbidden).detail("Only the owner can transfer ownership.")
                );
            }
            authorize(tx, actor, target)?;
            let demoted = tx
                .prepare_cached("UPDATE users SET role = 'admin' WHERE id = ?1 AND role = 'owner'")?
                .execute([actor])?;
            if demoted != 1 {
                return Err(Problem::new(Code::Internal));
            }
            let promoted = tx
                .prepare_cached(
                    "UPDATE users SET role = 'owner', reset_hash = NULL, reset_expires_at = NULL \
                     WHERE id = ?1 AND role = 'admin' AND status = 'active'",
                )?
                .execute([target])?;
            if promoted != 1 {
                return Err(Problem::invalid("Only an active admin can become the owner."));
            }
            tx.prepare_cached(
                "UPDATE users SET reset_hash = NULL, reset_expires_at = NULL \
                 WHERE role = 'admin' AND reset_hash IS NOT NULL",
            )?
            .execute([])?;
            audit::record(tx, Action::OwnershipTransferred, Some(actor), target)?;
            Ok::<_, Problem>(())
        })
        .await?;
        hub.broadcast(actor, &ServerMessage::AccessChanged);
        hub.broadcast(target, &ServerMessage::AccessChanged);
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
