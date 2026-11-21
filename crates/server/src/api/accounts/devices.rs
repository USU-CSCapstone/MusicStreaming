//! An account's devices, for admins: `adminListUserDevices`, `adminSignOutDevice` and
//! `adminSignOutDevices` (`requirements/users.md` §4, §8).

#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use super::authorize;
use crate::api::audit::{self, Action};
use crate::api::devices::{self, DeviceList, Which};
use crate::api::extract::Path;
use crate::api::playlists::settle;
use crate::api::realtime::Revoked;
use crate::api::{Caller, Hub, Id, Problem};
use crate::db::Database;

/// `adminListUserDevices`: the account's signed-in devices, most recently seen first.
pub async fn list(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Caller { user: actor, .. }: Caller,
    Path(Id(user)): Path<Id>,
) -> Result<Json<DeviceList>, Problem> {
    let mut items = db
        .try_read(move |conn| {
            authorize(conn, actor, user)?;
            Ok::<_, Problem>(devices::signed_in(conn, user)?)
        })
        .await?;
    devices::mark_connected(&hub, user, &mut items);
    Ok(Json(DeviceList { items }))
}

/// `adminSignOutDevice`: signs out one device of the account, `404` if it is not that
/// account's or not signed in. Recorded.
pub async fn sign_out(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Caller { user: actor, .. }: Caller,
    Path((Id(user), Id(device))): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    sign_out_of(db, hub, actor, user, Which::One(device)).await
}

/// `adminSignOutDevices`: signs out every signed-in device of the account. Recorded if there
/// was one.
pub async fn sign_out_all(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Caller { user: actor, .. }: Caller,
    Path(Id(user)): Path<Id>,
) -> Result<StatusCode, Problem> {
    sign_out_of(db, hub, actor, user, Which::All { keep: None }).await
}

/// Signs out `which` of `user`'s devices for `actor` and records it, then closes their
/// connections and sends the rest the new list.
async fn sign_out_of(
    db: Arc<Database>,
    hub: Arc<Hub>,
    actor: i64,
    user: i64,
    which: Which,
) -> Result<StatusCode, Problem> {
    // Settled: an admin who hangs up must not leave the connections open.
    settle(async move {
        let one = matches!(which, Which::One(_));
        let signed_out = db
            .try_write(move |tx| {
                authorize(tx, actor, user)?;
                let signed_out = devices::sign_out(tx, user, which, Revoked::LoggedOut)?;
                if one && signed_out.is_empty() {
                    return Err(Problem::not_found());
                }
                if !signed_out.is_empty() {
                    let device = signed_out.first().cloned().filter(|_| one);
                    audit::record(tx, Action::DevicesSignedOut { device }, Some(actor), user)?;
                }
                Ok::<_, Problem>(signed_out)
            })
            .await?;
        if signed_out.is_empty() {
            return Ok(());
        }
        for (device, _) in signed_out {
            hub.revoke(user, device, Revoked::LoggedOut);
        }
        devices::changed(db, hub, user);
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
