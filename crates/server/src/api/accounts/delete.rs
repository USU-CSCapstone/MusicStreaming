//! `adminDeleteUser`: deleting an account for good (`requirements/users.md` §9).

#[cfg(test)]
mod tests;

use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;

use super::authorize;
use crate::api::audit::{self, Action};
use crate::api::extract::{self, Path};
use crate::api::origin::Origin;
use crate::api::password::confirm;
use crate::api::playlists::settle;
use crate::api::realtime::Revoked;
use crate::api::{AppState, Caller, Id, Problem, invites};
use crate::db::now_ms;
use crate::db::retention::accounts::empty;

/// The spec's `DeleteUserRequest`: the admin's own current password.
#[derive(Deserialize)]
pub struct DeleteRequest {
    password: String,
}

/// `adminDeleteUser`: once the caller's password is confirmed, marks account `userId` as being
/// deleted, which ends its sessions, frees its name and hides it from every list, clears its
/// reset link and revokes its pending invites, and records `deleted` with its name, all in one
/// write that checks the caller and their rank again. Then closes its connections, drops what
/// memory holds for it, and starts emptying it at once, a chunk per write, rather than at the
/// next sweep. Its devices go with the rest: deleting them here would hold the writer while
/// each of its plays loses its device.
pub async fn delete(
    State(state): State<AppState>,
    caller: Caller,
    from: Origin,
    Path(Id(target)): Path<Id>,
    extract::Json(DeleteRequest { password }): extract::Json<DeleteRequest>,
) -> Result<StatusCode, Problem> {
    let actor = caller.user;
    let confirmed = confirm(&state, caller, &from, password).await?;
    let AppState { db, hub, players, queues, .. } = state;
    // Settled: an admin who hangs up drops this request, which must not leave the account's
    // connections open, or the account half emptied, once the deletion is committed.
    settle(async move {
        db.try_write(move |tx| {
            confirmed.check(tx)?;
            authorize(tx, actor, target)?;
            // Before the name is cleared, so the record keeps it.
            audit::record(tx, Action::Deleted, Some(actor), target)?;
            tx.prepare_cached(
                "UPDATE users SET status = 'deleting', username = NULL, reset_hash = NULL, \
                 reset_expires_at = NULL WHERE id = ?1",
            )?
            .execute([target])?;
            invites::revoke_pending_of(tx, target, now_ms())?;
            Ok::<_, Problem>(())
        })
        .await?;
        hub.revoke_all(target, Revoked::LoggedOut);
        players.forget(target);
        queues.forget(target);
        // What a request already past the gate gives memory later is saved while the account's
        // row is still there, then never written again, or dropped at the first save that finds
        // the row gone; neither is tried again.
        tokio::spawn(async move { empty(&db, target).await });
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
