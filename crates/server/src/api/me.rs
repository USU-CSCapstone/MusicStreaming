//! The caller's own account: `getMe` (`api/openapi.yaml`), and the checks on the names an
//! account and its devices are given.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::Connection;
use serde::Serialize;

use super::scope::Caller;
use super::{Id, Problem};
use crate::db::Database;

/// The spec's `User`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    id: Id,
    username: String,
    display_name: String,
    role: String,
    has_avatar: bool,
}

/// The account `id`, as the spec's `User`.
pub fn user(conn: &Connection, id: i64) -> rusqlite::Result<User> {
    conn.prepare_cached(
        "SELECT id, username, display_name, role, avatar_hash IS NOT NULL FROM users WHERE id = ?1",
    )?
    .query_row([id], |row| {
        Ok(User {
            id: Id(row.get(0)?),
            username: row.get(1)?,
            display_name: row.get(2)?,
            role: row.get(3)?,
            has_avatar: row.get(4)?,
        })
    })
}

/// `getMe`: the caller's own account.
pub async fn get(State(db): State<Arc<Database>>, caller: Caller) -> Result<Json<User>, Problem> {
    Ok(Json(db.read(move |conn| user(conn, caller.user)).await?))
}

/// Longer names are refused, so no one stores megabytes in an account or device row.
const MAX_NAME_CHARS: usize = 100;

/// Refuses a `field` that is blank or longer than [`MAX_NAME_CHARS`].
pub fn check_name(field: &str, value: &str) -> Result<(), Problem> {
    if value.trim().is_empty() || value.chars().count() > MAX_NAME_CHARS {
        return Err(Problem::invalid(format!("{field} must be 1 to {MAX_NAME_CHARS} characters")));
    }
    Ok(())
}

/// The spec's `Username`: 1 to 32 ASCII letters, digits, `_`, and `-`.
pub fn check_username(username: &str) -> Result<(), Problem> {
    let allowed = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    if (1..=32).contains(&username.len()) && username.bytes().all(allowed) {
        Ok(())
    } else {
        Err(Problem::invalid("username must be 1 to 32 letters, digits, '_', or '-'"))
    }
}
