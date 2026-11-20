//! Accounts, for admins: `adminListUsers`, `adminSetUserLibraries`, `adminChangeLibraryAccess`,
//! `adminSetUserStatus`, `adminDeleteUser` and the operations on an account's devices
//! (`api/openapi.yaml`, Admin). An admin sees every account, whatever its role
//! (`requirements/users.md` §8), and acts on those the rank rule allows ([`authorize`]). Admins
//! and the owner only, as everything under `/admin` is (`authenticate`).

pub(super) mod access;
mod authorize;
pub(super) mod delete;
pub(super) mod devices;
pub(super) mod status;
#[cfg(test)]
mod tests;

pub(super) use authorize::{authorize, manager_rank};

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::types::Value;
use rusqlite::{Connection, Row};
use serde::{Deserialize, Serialize};

use super::extract::Query;
use super::page::{Order, Page, Request, Sort, Source, Total, Unknown};
use super::time::serialize_rfc3339;
use super::{Id, Problem};
use crate::db::{Database, Json as Column, now_ms};

/// The accounts' order: by username, through its unique index.
const USERNAME: Sort =
    Sort { label: "accounts.username", columns: &["u.username"], unknown: Unknown::Never };

/// The columns [`from_row`] reads.
const SELECT: &[&str] = &[
    "u.id",
    "u.username",
    "u.display_name",
    "u.role",
    "u.avatar_hash IS NOT NULL",
    "u.status",
    "u.created_at",
    "(SELECT json_group_array(library_id) FROM \
        (SELECT library_id FROM library_access WHERE user_id = u.id ORDER BY library_id))",
    "u.reset_expires_at",
];

/// Whether an account can sign in.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Active,
    Suspended,
}

/// The spec's `AdminUser`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminUser {
    id: Id,
    username: String,
    display_name: String,
    role: String,
    has_avatar: bool,
    status: Status,
    #[serde(serialize_with = "serialize_rfc3339")]
    created_at: i64,
    library_ids: Vec<Id>,
    reset_pending: bool,
}

/// The row of [`SELECT`] as an account, as it stands at `now`. An account being deleted has no
/// name and is never read.
fn from_row(row: &Row, now: i64) -> rusqlite::Result<AdminUser> {
    let status = match row.get::<_, String>(5)?.as_str() {
        "suspended" => Status::Suspended,
        _ => Status::Active,
    };
    let reset_expires_at: Option<i64> = row.get(8)?;
    Ok(AdminUser {
        id: Id(row.get(0)?),
        username: row.get(1)?,
        display_name: row.get(2)?,
        role: row.get(3)?,
        has_avatar: row.get(4)?,
        status,
        created_at: row.get(6)?,
        library_ids: row.get::<_, Column<Vec<i64>>>(7)?.0.into_iter().map(Id).collect(),
        reset_pending: reset_expires_at.is_some_and(|expires| expires > now),
    })
}

/// Account `id` as the list shows it, as it stands now. One being deleted has no name, so the
/// caller has made sure it is not.
pub fn admin_user(conn: &Connection, id: i64) -> rusqlite::Result<AdminUser> {
    let now = now_ms();
    conn.prepare_cached(&format!("SELECT {} FROM users u WHERE u.id = ?1", SELECT.join(", ")))?
        .query_row([id], |row| from_row(row, now))
}

/// The accounts being listed: those not being deleted, and if `q` is given, those whose
/// username or display name holds it, whatever its case.
fn source(q: Option<String>) -> Source {
    let mut filter = "u.status <> 'deleting'".to_owned();
    let mut params = Vec::new();
    if let Some(q) = q.filter(|q| !q.is_empty()) {
        filter.push_str(
            " AND (instr(lower(u.username), lower(?)) > 0 \
             OR instr(lower(u.display_name), lower(?)) > 0)",
        );
        params = vec![Value::Text(q.clone()), Value::Text(q)];
    }
    Source {
        select: SELECT,
        from: "users u".to_owned(),
        joins: "",
        filter,
        params,
        total: Total::Counted,
        counting: None,
        back: None,
        walk: None,
    }
}

/// The parameters `adminListUsers` supports. Any other answers `422`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    cursor: Option<String>,
    limit: Option<u32>,
    q: Option<String>,
}

/// `adminListUsers`: every account by username, a page at a time.
pub async fn list(
    State(db): State<Arc<Database>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<AdminUser>>, Problem> {
    let request = Request::new(&USERNAME, Order::Asc, query.cursor, query.limit)?;
    let source = source(query.q);
    let now = now_ms();
    let page = db.read(move |conn| request.read(conn, &source, |row| from_row(row, now))).await?;
    Ok(Json(page))
}
