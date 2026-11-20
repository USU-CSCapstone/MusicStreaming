//! Invites, for admins: `adminCreateInvite`, `adminListInvites` and `adminRevokeInvite`
//! (`api/openapi.yaml`, Admin). An invite is a single-use code that creates an account with the
//! libraries chosen when it was made (`requirements/users.md` §2.2). Admins and the owner only,
//! as everything under `/admin` is (`authenticate`); every admin sees every invite.

pub(super) mod redeem;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::accounts::manager_rank;
use super::extract::{self, Path, Query};
use super::page::{Order, Page, Request, Sort, Source, Total, Unknown};
use super::session::{new_token, token_hash};
use super::time::{rfc3339, serialize_rfc3339};
use super::{Caller, Id, Problem};
use crate::db::{Database, Json as Column, new_id, now_ms};

/// How long an invite lasts when the request does not say, in days.
const DEFAULT_DAYS: i64 = 7;

/// The most days an invite may last.
const MAX_DAYS: i64 = 30;

/// Whether an invite is pending at `?2`: neither redeemed nor revoked, and not yet expired.
const PENDING: &str = "redeemed_at IS NULL AND revoked_at IS NULL AND expires_at > ?2";

const DAY_MS: i64 = 86_400_000;

/// The invites' order: newest first, through `invites_by_created`.
const CREATED: Sort =
    Sort { label: "invites.created", columns: &["i.created_at", "i.id"], unknown: Unknown::Never };

/// The columns [`invite`] reads. The accounts are joined only while they are live: one being
/// deleted is gone, as it is to every reader.
const SELECT: &[&str] = &[
    "i.id",
    "i.created_at",
    "i.expires_at",
    "i.redeemed_at",
    "i.revoked_at",
    "c.id",
    "c.username",
    "r.id",
    "r.username",
    "(SELECT json_group_array(library_id) FROM \
        (SELECT library_id FROM invite_libraries WHERE invite_id = i.id ORDER BY library_id))",
];

const JOINS: &str = "LEFT JOIN users c ON c.id = i.created_by AND c.status <> 'deleting' \
                     LEFT JOIN users r ON r.id = i.redeemed_by AND r.status <> 'deleting'";

/// Where an invite stands, derived from its times and never stored.
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Pending,
    Redeemed,
    Revoked,
    Expired,
}

/// An account as an invite names it.
#[derive(Serialize)]
struct Account {
    id: Id,
    username: String,
}

/// The spec's `Invite`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    id: Id,
    status: Status,
    created_by: Option<Account>,
    redeemed_by: Option<Account>,
    #[serde(serialize_with = "serialize_rfc3339")]
    created_at: i64,
    #[serde(serialize_with = "serialize_rfc3339")]
    expires_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    redeemed_at: Option<String>,
    library_ids: Vec<Id>,
}

/// The row of [`SELECT`] as an invite, as it stands at `now`.
fn invite(row: &Row, now: i64) -> rusqlite::Result<Invite> {
    let account = |first: usize| -> rusqlite::Result<Option<Account>> {
        let id: Option<i64> = row.get(first)?;
        let username: Option<String> = row.get(first + 1)?;
        Ok(id.zip(username).map(|(id, username)| Account { id: Id(id), username }))
    };
    let (expires_at, redeemed_at, revoked_at): (i64, Option<i64>, Option<i64>) =
        (row.get(2)?, row.get(3)?, row.get(4)?);
    let status = if redeemed_at.is_some() {
        Status::Redeemed
    } else if revoked_at.is_some() {
        Status::Revoked
    } else if expires_at <= now {
        Status::Expired
    } else {
        Status::Pending
    };
    let library_ids = row.get::<_, Column<Vec<i64>>>(9)?.0;
    Ok(Invite {
        id: Id(row.get(0)?),
        status,
        created_by: account(5)?,
        redeemed_by: account(7)?,
        created_at: row.get(1)?,
        expires_at,
        redeemed_at: redeemed_at.map(rfc3339),
        library_ids: library_ids.into_iter().map(Id).collect(),
    })
}

/// Invite `id`, as it stands at `now`, if there is one.
fn find(conn: &Connection, id: i64, now: i64) -> rusqlite::Result<Option<Invite>> {
    conn.prepare_cached(&format!(
        "SELECT {} FROM invites i {JOINS} WHERE i.id = ?1",
        SELECT.join(", ")
    ))?
    .query_row([id], |row| invite(row, now))
    .optional()
}

/// The body of `adminCreateInvite`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewInvite {
    library_ids: Vec<Id>,
    expires_in_days: Option<i64>,
}

/// `adminCreateInvite`'s answer: the invite and its code, which is never shown again.
#[derive(Serialize)]
pub struct Created {
    #[serde(flatten)]
    invite: Invite,
    code: String,
}

/// `adminCreateInvite`: makes an invite for the libraries, which must all exist, and answers it
/// with its code. Only the code's hash is stored. A creator who is no longer an active admin or
/// the owner when the invite is written is `403`, so no invite outlives their authority.
pub async fn create(
    State(db): State<Arc<Database>>,
    Caller { user, .. }: Caller,
    extract::Json(request): extract::Json<NewInvite>,
) -> Result<(StatusCode, Json<Created>), Problem> {
    let days = request.expires_in_days.unwrap_or(DEFAULT_DAYS);
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(Problem::invalid(format!("expiresInDays must be from 1 to {MAX_DAYS}")));
    }
    let code = new_token()?;
    let hash = token_hash(&code);
    let invite = db
        .try_write(move |tx| {
            manager_rank(tx, user)?;
            for &Id(library) in &request.library_ids {
                let exists = tx
                    .prepare_cached("SELECT 1 FROM libraries WHERE id = ?1")?
                    .exists([library])?;
                if !exists {
                    return Err(Problem::invalid("libraryIds names a library that does not exist"));
                }
            }
            let (id, now) = (new_id()?, now_ms());
            tx.prepare_cached(
                "INSERT INTO invites (id, code_hash, created_by, created_at, expires_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(rusqlite::params![id, hash, user, now, now + days * DAY_MS])?;
            let mut link = tx.prepare_cached(
                "INSERT OR IGNORE INTO invite_libraries (invite_id, library_id) VALUES (?1, ?2)",
            )?;
            for Id(library) in request.library_ids {
                link.execute([id, library])?;
            }
            Ok(find(tx, id, now)?)
        })
        .await?;
    let invite = invite.ok_or_else(|| Problem::new(super::Code::Internal))?;
    Ok((StatusCode::CREATED, Json(Created { invite, code })))
}

/// The parameters `adminListInvites` supports. Any other answers `422`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}

/// `adminListInvites`: every invite, newest first, a page at a time.
pub async fn list(
    State(db): State<Arc<Database>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<Invite>>, Problem> {
    let request = Request::new(&CREATED, Order::Desc, query.cursor, query.limit)?;
    let source = Source {
        select: SELECT,
        from: "invites i".to_owned(),
        joins: JOINS,
        filter: "1".to_owned(),
        params: Vec::<Value>::new(),
        total: Total::Counted,
        counting: None,
        back: None,
        walk: None,
    };
    let now = now_ms();
    let page = db.read(move |conn| request.read(conn, &source, |row| invite(row, now))).await?;
    Ok(Json(page))
}

/// `adminRevokeInvite`: revokes the invite if it is pending, and answers it as it stands now:
/// one already redeemed, revoked or expired is left as it was.
pub async fn revoke(
    State(db): State<Arc<Database>>,
    Path(Id(id)): Path<Id>,
) -> Result<Json<Invite>, Problem> {
    let invite = db
        .write(move |tx| {
            let now = now_ms();
            tx.prepare_cached(&format!(
                "UPDATE invites SET revoked_at = ?2 WHERE id = ?1 AND {PENDING}"
            ))?
            .execute([id, now])?;
            find(tx, id, now)
        })
        .await?;
    invite.map(Json).ok_or_else(Problem::not_found)
}

/// Revokes every invite `user` made that is pending at `now`, for when their authority to
/// invite ends: a link made under authority that no longer exists is not left live
/// (`requirements/users.md` §2.2).
pub fn revoke_pending_of(conn: &Connection, user: i64, now: i64) -> rusqlite::Result<()> {
    conn.prepare_cached(&format!(
        "UPDATE invites SET revoked_at = ?2 WHERE created_by = ?1 AND {PENDING}"
    ))?
    .execute([user, now])
    .map(drop)
}
