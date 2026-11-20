//! Transferring ownership.

use axum::Router;
use axum::http::StatusCode;
use futures_util::SinkExt;
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::api::AppState;
use crate::api::password::reset::issue_for_owner;
use crate::api::password::{hash_new, hold_checks};
use crate::api::testing::socket::{Socket, connect_with, next, next_of, serve};
use crate::api::testing::{
    TOKEN, add_device, add_user, app_with_tracks, audit, call, conn, eventually, hold_next,
    router_over, send_json, state,
};
use crate::db::now_ms;
use crate::db::testing::{User, mark_deleting};

const PASSWORD: &str = "correct horse battery staple";
const OWNER_ONLY: &str = "Only the owner can transfer ownership.";
const NOT_ACTIVE_ADMIN: &str = "Only an active admin can become the owner.";

/// The owner (user 1, [`TOKEN`]) over [`app_with_tracks`]; admins 2 (`admin-token`) and 3
/// (`admin3-token`); users 4 (`user-token`) and 5 (`other-token`), each of them with
/// [`PASSWORD`]; and user 6, being deleted. Answers the state too, to hold its writer.
async fn accounts() -> (tempfile::TempDir, AppState, Router) {
    let (temp, db, _app) = app_with_tracks().await;
    let state = state(&temp, db);
    let app = router_over(state.clone());
    let conn = conn(&temp);
    let password = hash_new(PASSWORD.to_owned(), Vec::new()).await.unwrap();
    conn.execute("UPDATE users SET password = ?1 WHERE id = 1", [&password]).unwrap();
    for (id, role) in [(2, "admin"), (3, "admin"), (4, "user"), (5, "user")] {
        User { password: password.clone(), ..User::new(id, role) }.insert(&conn).unwrap();
    }
    add_user(&conn, 6, "user");
    add_device(&conn, 2, 2, "admin-token");
    add_device(&conn, 3, 3, "admin3-token");
    add_device(&conn, 4, 4, "user-token");
    add_device(&conn, 5, 5, "other-token");
    mark_deleting(&conn, 6).unwrap();
    (temp, state, app)
}

/// [`accounts`] with a reset link pending for the owner and for each admin, so a refusal has
/// something to leave alone. The owner's is set by hand, since the host's reset records no
/// actor, which [`audit`] cannot read.
async fn accounts_with_links() -> (tempfile::TempDir, AppState, Router) {
    let (temp, state, app) = accounts().await;
    conn(&temp)
        .execute(
            "UPDATE users SET reset_hash = randomblob(32), reset_expires_at = ?1 WHERE id = 1",
            [now_ms() + 86_400_000],
        )
        .unwrap();
    issue_reset(&app, 2).await;
    issue_reset(&app, 3).await;
    (temp, state, app)
}

/// `transferOwnership` to account `user` as `token`, confirmed with `password`.
async fn transfer(app: &Router, token: &str, user: i64, password: &str) -> (StatusCode, Value) {
    let body = json!({ "userId": user.to_string(), "password": password });
    call(app, token, "POST", "/api/v1/admin/ownership-transfer", Some(body)).await
}

/// A reset link for account `user` issued by the owner, and its code.
async fn issue_reset(app: &Router, user: i64) -> String {
    let uri = format!("/api/v1/admin/users/{user}/password-reset");
    let (status, issued) =
        call(app, TOKEN, "POST", &uri, Some(json!({ "password": PASSWORD }))).await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    issued["code"].as_str().unwrap().to_owned()
}

/// The status `checkPasswordReset` answers for reset code `code`.
async fn check_reset(app: &Router, code: &str) -> StatusCode {
    let body = json!({ "code": code });
    send_json(app.clone(), "POST", "/api/v1/auth/password-resets/check", &body).await.0
}

/// Whether account `user`'s reset link is pending, as the list shows it to `token`'s account.
async fn reset_pending(app: &Router, token: &str, user: &str) -> Value {
    let (_, page) = call(app, token, "GET", "/api/v1/admin/users", None).await;
    let items = page["items"].as_array().unwrap();
    items.iter().find(|item| item["id"] == user).unwrap()["resetPending"].clone()
}

/// Every account's role.
fn roles(conn: &Connection) -> Vec<(i64, String)> {
    conn.prepare("SELECT id, role FROM users ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// What a transfer may touch: every account's role, status and reset link, and the audit log.
type Snapshot =
    (Vec<(i64, String, String, Option<Vec<u8>>, Option<i64>)>, Vec<(String, i64, i64, Value)>);

/// What a transfer may touch, as it stands in `conn`.
fn snapshot(conn: &Connection) -> Snapshot {
    let accounts = conn
        .prepare("SELECT id, role, status, reset_hash, reset_expires_at FROM users ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    (accounts, audit(conn))
}

/// Asserts that `conn` stands as `before`, with exactly one owner.
fn unchanged(conn: &Connection, before: &Snapshot) {
    assert_eq!(&snapshot(conn), before);
    let owners: i64 = conn
        .query_row("SELECT count(*) FROM users WHERE role = 'owner'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(owners, 1);
}

/// Asserts that a transfer to `user` as the owner is refused with `status`, `code` and
/// `detail`.
async fn refused(app: &Router, user: i64, status: StatusCode, code: &str, detail: Value) {
    let (answered, problem) = transfer(app, TOKEN, user, PASSWORD).await;
    assert_eq!((answered, &problem["code"]), (status, &json!(code)), "{user}: {problem}");
    assert_eq!(problem.get("detail").cloned().unwrap_or(Value::Null), detail, "{user}");
}

/// How many sign-in attempts are recorded, password confirmations among them.
fn attempts(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM login_attempts", [], |row| row.get(0)).unwrap()
}

/// Asserts that `socket` hears no `accessChanged` before it answers a ping.
async fn hears_no_change(socket: &mut Socket) {
    socket.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    loop {
        let message = next(socket).await;
        assert_ne!(message["type"], "accessChanged", "{message}");
        if message["type"] == "pong" {
            return;
        }
    }
}

#[tokio::test]
async fn the_admin_becomes_the_owner_and_the_owner_an_admin_whichever_id_is_larger() {
    let (temp, _state, app) = accounts().await;
    let conn = conn(&temp);

    assert_eq!(transfer(&app, TOKEN, 2, PASSWORD).await.0, StatusCode::NO_CONTENT);
    let after = roles(&conn);
    assert_eq!(&after[..3], [(1, "admin".into()), (2, "owner".into()), (3, "admin".into())]);

    assert_eq!(transfer(&app, "admin-token", 1, PASSWORD).await.0, StatusCode::NO_CONTENT);
    let after = roles(&conn);
    assert_eq!(&after[..3], [(1, "owner".into()), (2, "admin".into()), (3, "admin".into())]);
}

#[tokio::test]
async fn a_target_that_is_a_user_leaves_the_owner_as_it_was() {
    let (temp, _state, app) = accounts_with_links().await;
    let conn = conn(&temp);
    let before = snapshot(&conn);

    refused(
        &app,
        4,
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation_failed",
        json!(NOT_ACTIVE_ADMIN),
    )
    .await;
    unchanged(&conn, &before);
}

#[tokio::test]
async fn a_promotion_that_fails_leaves_the_owner_as_it_was() {
    let (temp, state, app) = accounts_with_links().await;
    let conn = conn(&temp);
    state
        .db
        .write(|tx| {
            tx.execute_batch(
                "CREATE TEMP TRIGGER refuse BEFORE UPDATE OF role ON users \
                 WHEN NEW.role = 'owner' BEGIN SELECT RAISE(ABORT, 'refused'); END",
            )
        })
        .await
        .unwrap();
    let before = snapshot(&conn);

    refused(&app, 2, StatusCode::INTERNAL_SERVER_ERROR, "internal", Value::Null).await;
    unchanged(&conn, &before);
}

#[tokio::test]
async fn a_target_not_an_active_admin_the_owner_may_act_on_changes_nothing() {
    let (temp, _state, app) = accounts_with_links().await;
    let conn = conn(&temp);
    conn.execute("UPDATE users SET status = 'suspended' WHERE id = 3", []).unwrap();
    let before = snapshot(&conn);

    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    for user in [4, 3] {
        refused(&app, user, unprocessable, "validation_failed", json!(NOT_ACTIVE_ADMIN)).await;
    }
    for user in [6, 99] {
        refused(&app, user, StatusCode::NOT_FOUND, "not_found", Value::Null).await;
    }
    let own = json!("You cannot do this to your own account.");
    refused(&app, 1, StatusCode::FORBIDDEN, "forbidden", own).await;
    unchanged(&conn, &before);
}

#[tokio::test]
async fn only_the_owner_with_their_password_may_transfer() {
    let (temp, _state, app) = accounts_with_links().await;
    let conn = conn(&temp);
    let before = snapshot(&conn);

    let (status, problem) = transfer(&app, "admin-token", 3, PASSWORD).await;
    assert_eq!((status, &problem["code"]), (StatusCode::FORBIDDEN, &json!("forbidden")));
    assert_eq!(problem["detail"], OWNER_ONLY);

    let (status, problem) = transfer(&app, "user-token", 2, PASSWORD).await;
    assert_eq!((status, &problem["code"]), (StatusCode::FORBIDDEN, &json!("forbidden")));
    assert_eq!(problem.get("detail"), None);

    let (status, problem) = transfer(&app, TOKEN, 2, "wrong horse").await;
    assert_eq!((status, &problem["code"]), (StatusCode::FORBIDDEN, &json!("wrong_password")));
    assert_eq!(problem["detail"], "Your current password is not correct.");
    unchanged(&conn, &before);
}

/// What happens between the owner's password check and their write.
enum Meanwhile {
    /// The owner hands ownership to admin 3, becoming an admin.
    Transferred,
    /// The owner's password changes.
    PasswordChanged,
    /// Admin 2, the target, is suspended.
    Suspended,
    /// Admin 2, the target, is marked as being deleted.
    Deleting,
    /// Admin 2, the target, is made a user.
    Demoted,
}

/// Transfers ownership to admin 2 with the owner's password check held while `meanwhile`
/// happens, and asserts that the request is refused with `status`, `code` and `detail`, and
/// changes nothing.
async fn transfer_stops_at(meanwhile: Meanwhile, status: StatusCode, code: &str, detail: Value) {
    let (temp, _state, app) = accounts_with_links().await;
    let conn = conn(&temp);
    let elsewhere = hash_new("vqp-mfrk-ztlw".to_owned(), Vec::new()).await.unwrap();
    let attempted = attempts(&conn);

    let held = hold_checks().await;
    let racing = tokio::spawn({
        let app = app.clone();
        async move { transfer(&app, TOKEN, 2, PASSWORD).await }
    });
    eventually("the attempt is recorded", async || (attempts(&conn) > attempted).then_some(()))
        .await;
    match meanwhile {
        Meanwhile::Transferred => conn.execute_batch(
            "UPDATE users SET role = 'admin' WHERE id = 1; \
             UPDATE users SET role = 'owner' WHERE id = 3;",
        ),
        Meanwhile::PasswordChanged => {
            conn.execute("UPDATE users SET password = ?1 WHERE id = 1", [&elsewhere]).map(drop)
        }
        Meanwhile::Suspended => {
            conn.execute("UPDATE users SET status = 'suspended' WHERE id = 2", []).map(drop)
        }
        Meanwhile::Deleting => mark_deleting(&conn, 2),
        Meanwhile::Demoted => {
            conn.execute("UPDATE users SET role = 'user' WHERE id = 2", []).map(drop)
        }
    }
    .unwrap();
    let before = snapshot(&conn);
    drop(held);

    let (answered, problem) = racing.await.unwrap();
    assert_eq!((answered, &problem["code"]), (status, &json!(code)), "{problem}");
    assert_eq!(problem.get("detail").cloned().unwrap_or(Value::Null), detail);
    unchanged(&conn, &before);
}

#[tokio::test]
async fn an_owner_who_transferred_ownership_meanwhile_transfers_nothing() {
    let detail = json!(OWNER_ONLY);
    transfer_stops_at(Meanwhile::Transferred, StatusCode::FORBIDDEN, "forbidden", detail).await;
}

#[tokio::test]
async fn an_owner_whose_password_changed_meanwhile_transfers_nothing() {
    let detail = json!("Your current password is not correct.");
    transfer_stops_at(Meanwhile::PasswordChanged, StatusCode::FORBIDDEN, "wrong_password", detail)
        .await;
}

#[tokio::test]
async fn an_admin_suspended_meanwhile_does_not_become_the_owner() {
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    let detail = json!(NOT_ACTIVE_ADMIN);
    transfer_stops_at(Meanwhile::Suspended, unprocessable, "validation_failed", detail).await;
}

#[tokio::test]
async fn an_admin_being_deleted_meanwhile_does_not_become_the_owner() {
    transfer_stops_at(Meanwhile::Deleting, StatusCode::NOT_FOUND, "not_found", Value::Null).await;
}

#[tokio::test]
async fn an_admin_made_a_user_meanwhile_does_not_become_the_owner() {
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    let detail = json!(NOT_ACTIVE_ADMIN);
    transfer_stops_at(Meanwhile::Demoted, unprocessable, "validation_failed", detail).await;
}

#[tokio::test]
async fn a_transfer_whose_request_is_dropped_mid_write_still_tells_both_accounts() {
    let (_temp, state, app) = accounts().await;
    let address = serve(app.clone()).await;
    let mut owner = connect_with(address, TOKEN).await;
    let mut admin = connect_with(address, "admin-token").await;

    let (held, release) = hold_next(&state.db, "AFTER UPDATE OF role ON users").await;
    let request = tokio::spawn({
        let app = app.clone();
        async move { transfer(&app, TOKEN, 2, PASSWORD).await }
    });
    held.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    drop(release);
    for socket in [&mut owner, &mut admin] {
        assert_eq!(next_of(socket, "accessChanged").await, json!({ "type": "accessChanged" }));
    }
}

#[tokio::test]
async fn a_transfer_ends_the_reset_link_issued_for_the_new_owner() {
    let (_temp, _state, app) = accounts().await;
    let code = issue_reset(&app, 2).await;
    assert_eq!(check_reset(&app, &code).await, StatusCode::OK);

    assert_eq!(transfer(&app, TOKEN, 2, PASSWORD).await.0, StatusCode::NO_CONTENT);
    assert_eq!(check_reset(&app, &code).await, StatusCode::NOT_FOUND);
    assert_eq!(reset_pending(&app, "admin-token", "2").await, false);
}

#[tokio::test]
async fn a_transfer_ends_the_reset_link_of_the_former_owner() {
    let (_temp, state, app) = accounts().await;
    let code = issue_for_owner(&state.db).await.unwrap().unwrap();
    assert_eq!(check_reset(&app, &code).await, StatusCode::OK);

    assert_eq!(transfer(&app, TOKEN, 2, PASSWORD).await.0, StatusCode::NO_CONTENT);
    assert_eq!(check_reset(&app, &code).await, StatusCode::NOT_FOUND);
    assert_eq!(reset_pending(&app, "admin-token", "1").await, false);
}

#[tokio::test]
async fn both_accounts_devices_alone_are_told() {
    let (_temp, _state, app) = accounts().await;
    let address = serve(app.clone()).await;
    let mut owner = connect_with(address, TOKEN).await;
    let mut admin = connect_with(address, "admin-token").await;
    let mut other_admin = connect_with(address, "admin3-token").await;
    let mut user = connect_with(address, "user-token").await;

    assert_eq!(transfer(&app, TOKEN, 2, PASSWORD).await.0, StatusCode::NO_CONTENT);
    for socket in [&mut owner, &mut admin] {
        assert_eq!(next_of(socket, "accessChanged").await, json!({ "type": "accessChanged" }));
    }
    hears_no_change(&mut other_admin).await;
    hears_no_change(&mut user).await;
}

#[tokio::test]
async fn the_transfer_is_recorded_and_a_refused_one_is_not() {
    let (temp, _state, app) = accounts().await;
    let conn = conn(&temp);

    assert_eq!(transfer(&app, TOKEN, 4, PASSWORD).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(audit(&conn), []);

    assert_eq!(transfer(&app, TOKEN, 2, PASSWORD).await.0, StatusCode::NO_CONTENT);
    assert_eq!(audit(&conn), [("ownership_transferred".to_owned(), 1, 2, json!({}))]);
}

#[tokio::test]
async fn the_former_owner_loses_the_owners_pages_and_the_new_owner_gains_them() {
    let (_temp, _state, app) = accounts().await;
    assert_eq!(transfer(&app, TOKEN, 2, PASSWORD).await.0, StatusCode::NO_CONTENT);

    let (status, problem) = call(&app, TOKEN, "GET", "/api/v1/admin/audit-log", None).await;
    assert_eq!((status, &problem["code"]), (StatusCode::FORBIDDEN, &json!("forbidden")));
    let (status, _) = call(&app, "admin-token", "GET", "/api/v1/admin/audit-log", None).await;
    assert_eq!(status, StatusCode::OK);
}
