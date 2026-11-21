//! `adminSetUserLibraries`: which libraries an account is granted (`requirements/users.md` §5).

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde::Deserialize;

use super::{AdminUser, admin_user, authorize};
use crate::api::audit::{self, Action};
use crate::api::extract::{self, Path};
use crate::api::realtime::ServerMessage;
use crate::api::{Caller, Hub, Id, Problem};
use crate::db::{Database, liked_songs, now_ms};

/// The body of `adminSetUserLibraries`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Libraries {
    library_ids: Vec<Id>,
}

/// `adminSetUserLibraries`: replaces the account's grants with exactly the libraries named,
/// which must all exist, and answers the account. What changed is recorded, and the account's
/// devices are told once it is committed; a request that changes nothing does neither.
pub async fn set(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Caller { user: actor, .. }: Caller,
    Path(Id(target)): Path<Id>,
    extract::Json(request): extract::Json<Libraries>,
) -> Result<Json<AdminUser>, Problem> {
    let wanted: BTreeSet<i64> = request.library_ids.into_iter().map(|Id(id)| id).collect();
    let (account, changed) = db
        .try_write(move |tx| {
            authorize(tx, actor, target)?;
            let mut exists = tx.prepare_cached("SELECT 1 FROM libraries WHERE id = ?1")?;
            for &library in &wanted {
                if !exists.exists([library])? {
                    return Err(Problem::invalid("libraryIds names a library that does not exist"));
                }
            }
            let held: BTreeSet<i64> = tx
                .prepare_cached("SELECT library_id FROM library_access WHERE user_id = ?1")?
                .query_map([target], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let granted: Vec<i64> = wanted.difference(&held).copied().collect();
            let revoked: Vec<i64> = held.difference(&wanted).copied().collect();
            let now = now_ms();
            let mut grant = tx.prepare_cached(
                "INSERT INTO library_access (user_id, library_id, granted_at) VALUES (?1, ?2, ?3)",
            )?;
            for &library in &granted {
                grant.execute([target, library, now])?;
            }
            let mut revoke = tx.prepare_cached(
                "DELETE FROM library_access WHERE user_id = ?1 AND library_id = ?2",
            )?;
            for &library in &revoked {
                revoke.execute([target, library])?;
            }
            if !granted.is_empty() {
                liked_songs::ensure(tx)?;
            }
            let changed = !granted.is_empty() || !revoked.is_empty();
            if changed {
                audit::record(tx, Action::AccessChanged { granted, revoked }, Some(actor), target)?;
            }
            Ok((admin_user(tx, target)?, changed))
        })
        .await?;
    if changed {
        hub.broadcast(target, &ServerMessage::AccessChanged);
    }
    Ok(Json(account))
}
