//! `adminSetUserLibraries` and `adminChangeLibraryAccess`: which libraries an account is
//! granted, one account at a time or many at once (`requirements/users.md` §5).

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;

use super::{AdminUser, admin_user, authorize};
use crate::api::audit::{self, Action};
use crate::api::extract::{self, Path};
use crate::api::realtime::ServerMessage;
use crate::api::{Caller, Hub, Id, Problem, playlists};
use crate::db::{Database, liked_songs, now_ms};
use rusqlite::Connection;

/// The most accounts one `adminChangeLibraryAccess` may name.
const MAX_ACCOUNTS: usize = 1000;

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
    let account = playlists::settle(async move {
        let (account, changed) = db
            .try_write(move |tx| {
                authorize(tx, actor, target)?;
                libraries_exist(tx, &wanted)?;
                let held = held(tx, target)?;
                let granted: Vec<i64> = wanted.difference(&held).copied().collect();
                let revoked: Vec<i64> = held.difference(&wanted).copied().collect();
                let grants = !granted.is_empty();
                let changed = apply(tx, actor, target, granted, revoked)?;
                if grants {
                    liked_songs::ensure(tx)?;
                }
                Ok::<_, Problem>((admin_user(tx, target)?, changed))
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

/// Fails with `422` unless every library in `libraries` exists.
fn libraries_exist(conn: &Connection, libraries: &BTreeSet<i64>) -> Result<(), Problem> {
    let mut exists = conn.prepare_cached("SELECT 1 FROM libraries WHERE id = ?1")?;
    for &library in libraries {
        if !exists.exists([library])? {
            return Err(Problem::invalid("libraryIds names a library that does not exist"));
        }
    }
    Ok(())
}

/// The libraries account `user` holds.
fn held(conn: &Connection, user: i64) -> rusqlite::Result<BTreeSet<i64>> {
    conn.prepare_cached("SELECT library_id FROM library_access WHERE user_id = ?1")?
        .query_map([user], |row| row.get(0))?
        .collect()
}

/// Grants `target` the libraries in `granted` and revokes those in `revoked` (each ascending,
/// the first not yet held and the second held), recording what changed. Answers whether
/// anything did; the caller tells the account's devices once the write commits.
fn apply(
    conn: &Connection,
    actor: i64,
    target: i64,
    granted: Vec<i64>,
    revoked: Vec<i64>,
) -> rusqlite::Result<bool> {
    let now = now_ms();
    let mut grant = conn.prepare_cached(
        "INSERT INTO library_access (user_id, library_id, granted_at) VALUES (?1, ?2, ?3)",
    )?;
    for &library in &granted {
        grant.execute([target, library, now])?;
    }
    let mut revoke =
        conn.prepare_cached("DELETE FROM library_access WHERE user_id = ?1 AND library_id = ?2")?;
    for &library in &revoked {
        revoke.execute([target, library])?;
    }
    let changed = !granted.is_empty() || !revoked.is_empty();
    if changed {
        audit::record(conn, Action::AccessChanged { granted, revoked }, Some(actor), target)?;
    }
    Ok(changed)
}

/// What `adminChangeLibraryAccess` does to each library.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Change {
    Grant,
    Revoke,
}

/// The body of `adminChangeLibraryAccess`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bulk {
    user_ids: Vec<Id>,
    library_ids: Vec<Id>,
    action: Change,
}

/// `adminChangeLibraryAccess`: grants or revokes the libraries named for every account named,
/// all or nothing. Each account that changed is recorded, and its devices are told once the
/// write is committed; an account already as asked is left alone.
pub async fn change(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Caller { user: actor, .. }: Caller,
    extract::Json(request): extract::Json<Bulk>,
) -> Result<StatusCode, Problem> {
    if request.user_ids.is_empty() {
        return Err(Problem::invalid("userIds must name at least one account"));
    }
    if request.user_ids.len() > MAX_ACCOUNTS {
        return Err(Problem::invalid(format!("userIds names more than {MAX_ACCOUNTS} accounts")));
    }
    let users: BTreeSet<i64> = request.user_ids.into_iter().map(|Id(id)| id).collect();
    let libraries: BTreeSet<i64> = request.library_ids.into_iter().map(|Id(id)| id).collect();
    let action = request.action;
    playlists::settle(async move {
        let changed = db
            .try_write(move |tx| {
                for &user in &users {
                    authorize(tx, actor, user)?;
                }
                libraries_exist(tx, &libraries)?;
                let mut granted_any = false;
                let mut changed = Vec::new();
                for &user in &users {
                    let held = held(tx, user)?;
                    let (granted, revoked): (Vec<i64>, Vec<i64>) = match action {
                        Change::Grant => {
                            (libraries.difference(&held).copied().collect(), Vec::new())
                        }
                        Change::Revoke => {
                            (Vec::new(), libraries.intersection(&held).copied().collect())
                        }
                    };
                    granted_any |= !granted.is_empty();
                    if apply(tx, actor, user, granted, revoked)? {
                        changed.push(user);
                    }
                }
                if granted_any {
                    liked_songs::ensure(tx)?;
                }
                Ok::<_, Problem>(changed)
            })
            .await?;
        for user in changed {
            hub.broadcast(user, &ServerMessage::AccessChanged);
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
