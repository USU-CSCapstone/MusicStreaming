//! The record of what was done to each account, and by whom (`requirements/users.md` §8): one
//! row of `admin_actions` per account affected, written in the transaction of the action itself
//! so the two commit or roll back together.

use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::db::now_ms;

/// An action the record keeps, with what it records about it.
pub enum Action {
    /// An invite made an account, with these libraries.
    AccountCreated { invite_id: i64, library_ids: Vec<i64> },
}

impl Action {
    /// The action's name in `admin_actions.action`.
    fn name(&self) -> &'static str {
        match self {
            Self::AccountCreated { .. } => "account_created",
        }
    }

    /// The row's `detail`, an object whose IDs are decimal strings, as the API shows them.
    fn detail(&self) -> Value {
        match self {
            Self::AccountCreated { invite_id, library_ids } => json!({
                "inviteId": invite_id.to_string(),
                "libraryIds": library_ids.iter().map(i64::to_string).collect::<Vec<_>>(),
            }),
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
