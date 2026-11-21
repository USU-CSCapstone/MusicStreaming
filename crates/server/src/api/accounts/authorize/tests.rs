//! The rank rule, for every pairing of roles.

use rusqlite::Connection;

use super::authorize;
use crate::api::testing::conn;
use crate::db::testing::{User, mark_deleting, open};

const OWNER_ONLY: &str = "No one can do this to the owner's account.";
const SELF: &str = "You cannot do this to your own account.";
const ADMIN: &str = "Only the owner can do this to an admin's account.";
const NOT_ADMIN: &str = "Only an admin or the owner can manage accounts.";

/// The owner (1), admins 2 and 3, users 4 and 5 (suspended), and user 6, being deleted.
fn accounts() -> (tempfile::TempDir, Connection) {
    let (temp, _db) = open();
    let conn = conn(&temp);
    for (id, role) in
        [(1, "owner"), (2, "admin"), (3, "admin"), (4, "user"), (5, "user"), (6, "user")]
    {
        User::new(id, role).insert(&conn).unwrap();
    }
    conn.execute("UPDATE users SET status = 'suspended' WHERE id = 5", []).unwrap();
    mark_deleting(&conn, 6).unwrap();
    (temp, conn)
}

/// What `authorize` answers: `200` for leave to go ahead, else the Problem's status and detail.
/// A `403` is always `forbidden`.
fn answer(conn: &Connection, actor: i64, target: i64) -> (u16, Option<String>) {
    match authorize(conn, actor, target) {
        Ok(_) => (200, None),
        Err(problem) => {
            let problem = serde_json::to_value(problem).unwrap();
            if problem["status"] == 403 {
                assert_eq!(problem["code"], "forbidden");
            }
            let detail = problem["detail"].as_str().map(str::to_owned);
            (problem["status"].as_u64().unwrap() as u16, detail)
        }
    }
}

#[test]
fn each_role_acts_only_on_those_it_outranks() {
    let (_temp, conn) = accounts();
    let rows: &[(i64, i64, u16, Option<&str>)] = &[
        // The owner
        (1, 1, 403, Some(SELF)),
        (1, 2, 200, None),
        (1, 4, 200, None),
        (1, 5, 200, None),
        (1, 6, 404, None),
        (1, 99, 404, None),
        // An admin
        (2, 1, 403, Some(OWNER_ONLY)),
        (2, 2, 403, Some(SELF)),
        (2, 3, 403, Some(ADMIN)),
        (2, 4, 200, None),
        (2, 5, 200, None),
        (2, 6, 404, None),
        (2, 99, 404, None),
        // A user
        (4, 1, 403, Some(NOT_ADMIN)),
        (4, 2, 403, Some(NOT_ADMIN)),
        (4, 4, 403, Some(NOT_ADMIN)),
        (4, 5, 403, Some(NOT_ADMIN)),
        (4, 6, 403, Some(NOT_ADMIN)),
        (4, 99, 403, Some(NOT_ADMIN)),
    ];
    for &(actor, target, status, detail) in rows {
        let expected = (status, detail.map(str::to_owned));
        assert_eq!(answer(&conn, actor, target), expected, "{actor} on {target}");
    }
}

#[test]
fn an_actor_demoted_suspended_or_deleted_since_the_gate_is_refused() {
    for (change, refused) in [
        ("UPDATE users SET role = 'user' WHERE id = 2", (403, Some(NOT_ADMIN.to_owned()))),
        ("UPDATE users SET status = 'suspended' WHERE id = 2", (401, None)),
        ("UPDATE users SET status = 'deleting', username = NULL WHERE id = 2", (401, None)),
        ("DELETE FROM users WHERE id = 2", (401, None)),
    ] {
        let (_temp, conn) = accounts();
        assert_eq!(answer(&conn, 2, 4), (200, None));
        conn.execute(change, []).unwrap();
        for target in [4, 99] {
            assert_eq!(answer(&conn, 2, target), refused, "{change}, on {target}");
        }
    }
}
