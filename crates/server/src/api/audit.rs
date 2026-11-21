//! The record of what was done to each account, and by whom (`requirements/users.md` §8): one
//! row of `admin_actions` per account affected, written in the transaction of the action itself
//! so the two commit or roll back together.

#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::types::Value as Param;
use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::extract::Query;
use super::page::{Order, Page, Request, Sort, Source, Total, Unknown};
use super::time::serialize_rfc3339;
use super::{Caller, Code, Id, Problem};
use crate::db::{Database, Json as Column, now_ms};

/// The record's order: newest first, which the row's ID follows, as it rises with time.
const NEWEST: Sort = Sort { label: "audit.id", columns: &["a.id"], unknown: Unknown::Never };

/// The columns [`entry`] reads. An account is `deleted` unless its stored ID names one that is
/// not being deleted; the ID alone decides, so a name taken later by someone else changes nothing.
const SELECT: &[&str] = &[
    "a.id",
    "a.at",
    "a.action",
    "a.actor_id",
    "a.actor_name",
    "NOT EXISTS (SELECT 1 FROM users WHERE id = a.actor_id AND status <> 'deleting')",
    "a.target_id",
    "a.target_name",
    "NOT EXISTS (SELECT 1 FROM users WHERE id = a.target_id AND status <> 'deleting')",
    "a.detail",
];

/// An action the record keeps, with what it records about it.
pub enum Action {
    /// An invite made an account, with these libraries.
    AccountCreated { invite_id: i64, library_ids: Vec<i64> },
    /// An account was granted these libraries and lost these, each list ascending.
    AccessChanged { granted: Vec<i64>, revoked: Vec<i64> },
    /// An account was suspended.
    Suspended,
    /// A suspended account was restored.
    Restored,
    /// An account's devices were signed out: the one `(id, name)`, or all of them if none.
    DevicesSignedOut { device: Option<(i64, String)> },
    /// A password reset link was issued for an account.
    PasswordResetIssued,
    /// An account's password was set with its reset link.
    PasswordResetUsed,
    /// An account's role was changed, from and to `admin` or `user`.
    RoleChanged { from: &'static str, to: &'static str },
    /// The actor handed ownership to the account.
    OwnershipTransferred,
    /// An account was deleted.
    Deleted,
}

impl Action {
    /// The action's name in `admin_actions.action`.
    fn name(&self) -> &'static str {
        match self {
            Self::AccountCreated { .. } => "account_created",
            Self::AccessChanged { .. } => "access_changed",
            Self::Suspended => "suspended",
            Self::Restored => "restored",
            Self::DevicesSignedOut { .. } => "devices_signed_out",
            Self::PasswordResetIssued => "password_reset_issued",
            Self::PasswordResetUsed => "password_reset_used",
            Self::RoleChanged { .. } => "role_changed",
            Self::OwnershipTransferred => "ownership_transferred",
            Self::Deleted => "deleted",
        }
    }

    /// The row's `detail`, an object whose IDs are decimal strings, as the API shows them.
    fn detail(&self) -> Value {
        let ids = |ids: &[i64]| ids.iter().map(i64::to_string).collect::<Vec<_>>();
        match self {
            Self::AccountCreated { invite_id, library_ids } => json!({
                "inviteId": invite_id.to_string(),
                "libraryIds": ids(library_ids),
            }),
            Self::AccessChanged { granted, revoked } => json!({
                "granted": ids(granted),
                "revoked": ids(revoked),
            }),
            Self::Suspended
            | Self::Restored
            | Self::PasswordResetIssued
            | Self::PasswordResetUsed
            | Self::OwnershipTransferred
            | Self::Deleted => json!({}),
            Self::DevicesSignedOut { device: Some((id, name)) } => {
                json!({ "deviceId": id.to_string(), "deviceName": name })
            }
            Self::DevicesSignedOut { device: None } => json!({ "all": true }),
            Self::RoleChanged { from, to } => json!({ "from": from, "to": to }),
        }
    }
}

/// Records `action`, done by `actor` (none for what no one signed in did) to `target`, now. Both
/// names are read as they are in `conn`, so the row keeps them after either account is gone. An
/// actor being deleted, or no longer there, keeps its ID but has no name.
pub fn record(
    conn: &Connection,
    action: Action,
    actor: Option<i64>,
    target: i64,
) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "INSERT INTO admin_actions \
             (at, action, actor_id, actor_name, target_id, target_name, detail) \
         VALUES (?1, ?2, ?3, (SELECT username FROM users WHERE id = ?3), \
                 ?4, (SELECT username FROM users WHERE id = ?4), ?5)",
    )?
    .execute(params![now_ms(), action.name(), actor, target, action.detail().to_string()])
    .map(drop)
}

/// An account as the record named it.
#[derive(Serialize)]
pub struct AuditAccount {
    id: Id,
    /// Its name when the row was written; none for an actor already being deleted then.
    username: Option<String>,
    /// Whether the account no longer exists.
    deleted: bool,
}

/// The spec's `AuditEntry`.
#[derive(Serialize)]
pub struct AuditEntry {
    id: Id,
    #[serde(serialize_with = "serialize_rfc3339")]
    at: i64,
    action: String,
    actor: Option<AuditAccount>,
    target: AuditAccount,
    detail: Value,
}

/// The entry in `row`, which is [`SELECT`]'s columns.
fn entry(row: &Row) -> rusqlite::Result<AuditEntry> {
    let actor: Option<i64> = row.get(3)?;
    Ok(AuditEntry {
        id: Id(row.get(0)?),
        at: row.get(1)?,
        action: row.get(2)?,
        actor: match actor {
            Some(id) => {
                Some(AuditAccount { id: Id(id), username: row.get(4)?, deleted: row.get(5)? })
            }
            None => None,
        },
        target: AuditAccount {
            id: Id(row.get(6)?),
            username: Some(row.get(7)?),
            deleted: row.get(8)?,
        },
        detail: row.get::<_, Column<Value>>(9)?.0,
    })
}

/// The query of [`list`]. Any other parameter answers `422`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}

/// `adminListAuditLog`: every entry, newest first, a page at a time. The owner's alone.
pub async fn list(
    State(db): State<Arc<Database>>,
    Caller { owner, .. }: Caller,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<AuditEntry>>, Problem> {
    if !owner {
        return Err(Problem::new(Code::Forbidden).detail("Only the owner can read the audit log."));
    }
    let request = Request::new(&NEWEST, Order::Desc, query.cursor, query.limit)?;
    let source = Source {
        select: SELECT,
        from: "admin_actions a".to_owned(),
        joins: "",
        filter: "1".to_owned(),
        params: Vec::<Param>::new(),
        total: Total::Counted,
        counting: None,
        back: None,
        walk: None,
    };
    Ok(Json(db.read(move |conn| request.read(conn, &source, entry)).await?))
}
