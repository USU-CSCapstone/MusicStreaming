//! The rank rule (`requirements/users.md` §1): an admin manages users only, the owner manages
//! admins and users, and no one manages the owner or themselves.

#[cfg(test)]
mod tests;

use rusqlite::{Connection, OptionalExtension};

use crate::api::{Code, Problem};

/// The account an action may go ahead on.
pub struct Target {
    /// Whether it is suspended.
    pub suspended: bool,
}

/// How far a role reaches: an account manages only those whose role ranks strictly below.
fn rank(role: &str) -> u8 {
    match role {
        "owner" => 2,
        "admin" => 1,
        _ => 0,
    }
}

/// A `403 forbidden` that says why.
fn forbidden(detail: &str) -> Problem {
    Problem::new(Code::Forbidden).detail(detail)
}

/// Account `id`'s role and status, if it exists.
fn account(conn: &Connection, id: i64) -> rusqlite::Result<Option<(String, String)>> {
    conn.prepare_cached("SELECT role, status FROM users WHERE id = ?1")?
        .query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()
}

/// `actor`'s rank, as it stands in `conn`, if they are an active admin or the owner, else `403`.
/// Call it inside the action's write, since the gate's view of the actor may be stale by then.
pub fn manager_rank(conn: &Connection, actor: i64) -> Result<u8, Problem> {
    match account(conn, actor)? {
        Some((role, status)) if status == "active" && rank(&role) > 0 => Ok(rank(&role)),
        _ => Err(forbidden("Only an active admin or the owner can manage accounts.")),
    }
}

/// Whether `actor` may act on account `target`, as both stand in `conn`: call it inside the
/// action's write, since the gate's view of the actor may be stale by then. An actor who is not
/// an active admin or owner is `403` whatever the target ([`manager_rank`]); a target that does
/// not exist or is being deleted is `404`; a target the actor does not outrank is `403`.
pub fn authorize(conn: &Connection, actor: i64, target: i64) -> Result<Target, Problem> {
    let actor_rank = manager_rank(conn, actor)?;
    let (target_role, target_status) = match account(conn, target)? {
        Some((role, status)) if status != "deleting" => (role, status),
        _ => return Err(Problem::not_found()),
    };
    if actor == target {
        Err(forbidden("You cannot do this to your own account."))
    } else if target_role == "owner" {
        Err(forbidden("No one can do this to the owner's account."))
    } else if actor_rank <= rank(&target_role) {
        Err(forbidden("Only the owner can do this to an admin's account."))
    } else {
        Ok(Target { suspended: target_status == "suspended" })
    }
}
