//! `adminSetUserRole`: the owner makes a user an admin or an admin a user
//! (`requirements/users.md` §1, §8).

#[cfg(test)]
mod tests;

use axum::Json;
use axum::extract::State;
use rusqlite::params;
use serde::Deserialize;

use super::authorize::OWNER;
use super::{AdminUser, admin_user, authorize, manager_rank};
use crate::api::audit::{self, Action};
use crate::api::extract::{self, Path};
use crate::api::origin::Origin;
use crate::api::password::confirm;
use crate::api::playlists::settle;
use crate::api::realtime::ServerMessage;
use crate::api::{AppState, Caller, Code, Id, Problem, invites};
use crate::db::{liked_songs, now_ms};

/// A role this request may give: never `owner`, which only a transfer moves.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    Admin,
    User,
}

impl Role {
    /// The role's name in `users.role`.
    fn name(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
        }
    }
}

/// The spec's `UserRoleChange`: the role to give and the owner's own current password.
#[derive(Deserialize)]
pub struct RoleChange {
    role: Role,
    password: String,
}

/// `adminSetUserRole`: once the caller's password is confirmed, gives account `userId` the role
/// in one write that checks again that the caller is the owner, with that password, and that the
/// account is one they may act on and not suspended. The change clears the account's reset link,
/// which an admin may have issued while it was a user; a demotion revokes its pending invites,
/// and a promotion gives it a Liked Songs in every library it now reaches. Its granted libraries
/// are kept, so a demoted admin returns to them. Recorded, and the account's devices are told
/// once it is committed; the role it already has changes nothing.
pub async fn set(
    State(state): State<AppState>,
    caller: Caller,
    from: Origin,
    Path(Id(target)): Path<Id>,
    extract::Json(RoleChange { role, password }): extract::Json<RoleChange>,
) -> Result<Json<AdminUser>, Problem> {
    let actor = caller.user;
    let confirmed = confirm(&state, caller, &from, password).await?;
    let AppState { db, hub, .. } = state;
    // Settled: an owner who hangs up drops this request, which must not leave the account's
    // devices untold once the change is committed.
    let account = settle(async move {
        let (account, changed) = db
            .try_write(move |tx| {
                confirmed.check(tx)?;
                if manager_rank(tx, actor)? != OWNER {
                    return Err(Problem::new(Code::Forbidden)
                        .detail("Only the owner can change an account's role."));
                }
                let account = authorize(tx, actor, target)?;
                if account.suspended {
                    return Err(Problem::invalid("Restore this account before changing its role."));
                }
                let to = role.name();
                if account.role == to {
                    return Ok((admin_user(tx, target)?, false));
                }
                tx.prepare_cached(
                    "UPDATE users SET role = ?2, reset_hash = NULL, reset_expires_at = NULL \
                     WHERE id = ?1",
                )?
                .execute(params![target, to])?;
                let from = match role {
                    Role::Admin => {
                        liked_songs::ensure(tx)?;
                        Role::User
                    }
                    Role::User => {
                        invites::revoke_pending_of(tx, target, now_ms())?;
                        Role::Admin
                    }
                };
                audit::record(
                    tx,
                    Action::RoleChanged { from: from.name(), to },
                    Some(actor),
                    target,
                )?;
                Ok::<_, Problem>((admin_user(tx, target)?, true))
            })
            .await?;
        if changed {
            hub.broadcast(target, &ServerMessage::AccessChanged);
        }
        Ok(account)
    })
    .await?;
    Ok(Json(account))
}
