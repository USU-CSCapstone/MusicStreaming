//! `adminSetUserStatus`: suspending and restoring an account (`requirements/users.md` §9).

#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde::Deserialize;

use super::{AdminUser, Status, admin_user, authorize};
use crate::api::audit::{self, Action};
use crate::api::devices::{self, Which};
use crate::api::extract::{self, Path};
use crate::api::playlists::settle;
use crate::api::realtime::Revoked;
use crate::api::{Caller, Hub, Id, Problem, invites};
use crate::db::{Database, now_ms};

/// The body of `adminSetUserStatus`.
#[derive(Deserialize)]
pub struct StatusChange {
    status: Status,
}

/// `adminSetUserStatus`: suspends or restores the account and answers it. Suspending signs out
/// every device, clears its reset link and revokes its pending invites in the same write, then
/// closes its realtime connections; restoring only lets it sign in again, so no earlier session
/// returns. Each is recorded; setting the status it already has changes nothing.
pub async fn set(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Caller { user: actor, .. }: Caller,
    Path(Id(target)): Path<Id>,
    extract::Json(StatusChange { status }): extract::Json<StatusChange>,
) -> Result<Json<AdminUser>, Problem> {
    // Settled: an admin who hangs up must not leave the account's connections open.
    let account = settle(async move {
        let (account, suspended) = db
            .try_write(move |tx| {
                let was_suspended = authorize(tx, actor, target)?.suspended;
                let suspended = match status {
                    Status::Suspended if !was_suspended => {
                        tx.prepare_cached(
                            "UPDATE users SET status = 'suspended', reset_hash = NULL, \
                             reset_expires_at = NULL WHERE id = ?1",
                        )?
                        .execute([target])?;
                        devices::sign_out(
                            tx,
                            target,
                            Which::All { keep: None },
                            Revoked::LoggedOut,
                        )?;
                        invites::revoke_pending_of(tx, target, now_ms())?;
                        audit::record(tx, Action::Suspended, Some(actor), target)?;
                        true
                    }
                    Status::Active if was_suspended => {
                        tx.prepare_cached("UPDATE users SET status = 'active' WHERE id = ?1")?
                            .execute([target])?;
                        audit::record(tx, Action::Restored, Some(actor), target)?;
                        false
                    }
                    _ => false,
                };
                Ok::<_, Problem>((admin_user(tx, target)?, suspended))
            })
            .await?;
        if suspended {
            hub.revoke_all(target, Revoked::LoggedOut);
        }
        Ok(account)
    })
    .await?;
    Ok(Json(account))
}
