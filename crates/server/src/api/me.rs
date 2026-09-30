//! The caller's own account: `getMe` (`api/openapi.yaml`).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::Connection;
use serde::Serialize;

use super::authenticate::Caller;
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

pub async fn get(State(db): State<Arc<Database>>, caller: Caller) -> Result<Json<User>, Problem> {
    Ok(Json(db.read(move |conn| user(conn, caller.user)).await?))
}
