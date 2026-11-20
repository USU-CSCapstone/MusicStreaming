//! Promoting and demoting accounts.

use axum::Router;
use axum::http::StatusCode;
use futures_util::SinkExt;
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::api::AppState;
use crate::api::password::{hash_new, hold_checks};
use crate::api::testing::socket::{Socket, connect_with, next, next_of, serve};
use crate::api::testing::{
    TOKEN, add_device, add_user, app_with_tracks, audit, call, conn, eventually, hold_next,
    router_over, send_json, state,
};
use crate::db::testing::{User, access, mark_deleting};

const PASSWORD: &str = "correct horse battery staple";
const OWNER_ONLY: &str = "Only the owner can change an account's role.";
const SUSPENDED: &str = "Restore this account before changing its role.";

/// The owner (user 1, [`TOKEN`]) over [`app_with_tracks`], whose libraries are 1 and 2; admins
/// 2 (`admin-token`) and 3 (`admin3-token`); user 4 granted library 1, signed in on devices 4
/// (`user-token`) and 7 (`phone-token`); user 5 (`other-token`), each of them with
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
    add_device(&conn, 4, 7, "phone-token");
    add_device(&conn, 5, 5, "other-token");
    access(&conn, 4, 1).unwrap();
    mark_deleting(&conn, 6).unwrap();
    (temp, state, app)
}

/// `adminSetUserRole` on account `user` as `token`, giving `role`, confirmed with `password`.
async fn set(
    app: &Router,
    token: &str,
    user: i64,
    role: &str,
    password: &str,
) -> (StatusCode, Value) {
    let uri = format!("/api/v1/admin/users/{user}/role");
    call(app, token, "PUT", &uri, Some(json!({ "role": role, "password": password }))).await
}

/// A reset link for account `user` issued by `token`'s account, and its code.
async fn issue_reset(app: &Router, token: &str, user: i64) -> String {
    let uri = format!("/api/v1/admin/users/{user}/password-reset");
    let (status, issued) =
        call(app, token, "POST", &uri, Some(json!({ "password": PASSWORD }))).await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    issued["code"].as_str().unwrap().to_owned()
}

/// A pending invite to library 1 made by `token`'s account, and its ID.
async fn invite(app: &Router, token: &str) -> String {
    let body = json!({ "libraryIds": ["1"] });
    let (status, created) = call(app, token, "POST", "/api/v1/admin/invites", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().unwrap().to_owned()
}

/// The status of invite `id` as the admins' list shows it.
async fn invite_status(app: &Router, id: &str) -> Value {
    let (_, page) = call(app, TOKEN, "GET", "/api/v1/admin/invites", None).await;
    let items = page["items"].as_array().unwrap();
    items.iter().find(|item| item["id"] == id).unwrap()["status"].clone()
}

/// The status `checkPasswordReset` answers for reset code `code`.
async fn check_reset(app: &Router, code: &str) -> StatusCode {
    let body = json!({ "code": code });
    send_json(app.clone(), "POST", "/api/v1/auth/password-resets/check", &body).await.0
}

/// Asserts that reset code `code` is no longer live: checking and redeeming it are `404`.
async fn reset_ended(app: &Router, code: &str) {
    assert_eq!(check_reset(app, code).await, StatusCode::NOT_FOUND);
    let body = json!({ "code": code, "password": "vqp-mfrk-ztlw" });
    let redeemed = send_json(app.clone(), "POST", "/api/v1/auth/password-resets/redeem", &body);
    assert_eq!(redeemed.await.0, StatusCode::NOT_FOUND);
}

/// What a role change may touch: every account's role, status and reset link, every invite's
/// revocation, and the audit log.
type Snapshot = (
    Vec<(i64, String, String, Option<Vec<u8>>, Option<i64>)>,
    Vec<Option<i64>>,
    Vec<(String, i64, i64, Value)>,
);

/// What a role change may touch, as it stands in `conn`.
fn snapshot(conn: &Connection) -> Snapshot {
    let accounts = conn
        .prepare("SELECT id, role, status, reset_hash, reset_expires_at FROM users ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let invites = conn
        .prepare("SELECT revoked_at FROM invites ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    (accounts, invites, audit(conn))
}

/// [`accounts`] with a reset link pending for user 4, issued by admin 2, and for admin 3,
/// issued by the owner, and a pending invite from each admin, so a refusal has something to
/// leave alone.
async fn accounts_with_links() -> (tempfile::TempDir, AppState, Router) {
    let (temp, state, app) = accounts().await;
    issue_reset(&app, "admin-token", 4).await;
    issue_reset(&app, TOKEN, 3).await;
    invite(&app, "admin-token").await;
    invite(&app, "admin3-token").await;
    (temp, state, app)
}

/// The libraries `token`'s account reaches.
async fn reached(app: &Router, token: &str) -> Vec<Value> {
    let (_, list) = call(app, token, "GET", "/api/v1/libraries", None).await;
    list["items"].as_array().unwrap().iter().map(|library| library["id"].clone()).collect()
}

/// How many Liked Songs account `user` has in each of libraries 1 and 2.
fn liked(conn: &Connection, user: i64) -> [i64; 2] {
    [1, 2].map(|library| {
        conn.query_row(
            "SELECT count(*) FROM playlists WHERE user_id = ?1 AND library_id = ?2 \
             AND kind = 'liked'",
            [user, library],
            |row| row.get(0),
        )
        .unwrap()
    })
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
async fn only_the_owner_with_their_password_may_change_a_role() {
    let (temp, _state, app) = accounts_with_links().await;
    let conn = conn(&temp);
    let before = snapshot(&conn);

    for (user, role) in [(4, "admin"), (3, "user")] {
        let (status, problem) = set(&app, "admin-token", user, role, PASSWORD).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{user}: {problem}");
        assert_eq!(problem["code"], "forbidden", "{user}");
        assert_eq!(problem["detail"], OWNER_ONLY, "{user}");
    }

    let (status, problem) = set(&app, "user-token", 5, "admin", PASSWORD).await;
    assert_eq!((status, &problem["code"]), (StatusCode::FORBIDDEN, &json!("forbidden")));
    assert_eq!(problem.get("detail"), None);

    let (status, problem) = set(&app, TOKEN, 4, "admin", "wrong horse").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    assert_eq!(problem["code"], "wrong_password");
    assert_eq!(problem["detail"], "Your current password is not correct.");

    for user in [99, 6] {
        let (status, problem) = set(&app, TOKEN, user, "admin", PASSWORD).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{user}: {problem}");
        assert_eq!(
            problem,
            json!({ "type": "about:blank", "title": "Not Found", "status": 404, "code": "not_found" })
        );
    }
    assert_eq!(snapshot(&conn), before);
}

/// What happens between the owner's password check and their write.
enum Meanwhile {
    /// The owner hands ownership to admin 3, becoming an admin.
    Transferred,
    /// The owner's password changes.
    PasswordChanged,
    /// User 4, being promoted, is suspended.
    Suspended,
    /// User 4, being promoted, is marked as being deleted.
    Deleting,
}

/// Promotes user 4 as the owner with their password check held while `meanwhile` happens, and
/// asserts that the request is refused with `status`, `code` and `detail`, and changes nothing.
async fn promote_stops_at(meanwhile: Meanwhile, status: StatusCode, code: &str, detail: Value) {
    let (temp, _state, app) = accounts_with_links().await;
    let conn = conn(&temp);
    let elsewhere = hash_new("vqp-mfrk-ztlw".to_owned(), Vec::new()).await.unwrap();
    let attempted = attempts(&conn);

    let held = hold_checks().await;
    let racing = tokio::spawn({
        let app = app.clone();
        async move { set(&app, TOKEN, 4, "admin", PASSWORD).await }
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
            conn.execute("UPDATE users SET status = 'suspended' WHERE id = 4", []).map(drop)
        }
        Meanwhile::Deleting => mark_deleting(&conn, 4),
    }
    .unwrap();
    let before = snapshot(&conn);
    drop(held);

    let (answered, problem) = racing.await.unwrap();
    assert_eq!((answered, &problem["code"]), (status, &json!(code)), "{problem}");
    assert_eq!(problem.get("detail").cloned().unwrap_or(Value::Null), detail);
    assert_eq!(snapshot(&conn), before);
}

#[tokio::test]
async fn an_owner_who_transferred_ownership_meanwhile_changes_no_role() {
    let detail = json!(OWNER_ONLY);
    promote_stops_at(Meanwhile::Transferred, StatusCode::FORBIDDEN, "forbidden", detail).await;
}

#[tokio::test]
async fn an_owner_whose_password_changed_meanwhile_changes_no_role() {
    let detail = json!("Your current password is not correct.");
    promote_stops_at(Meanwhile::PasswordChanged, StatusCode::FORBIDDEN, "wrong_password", detail)
        .await;
}

#[tokio::test]
async fn an_account_suspended_meanwhile_keeps_its_role() {
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    promote_stops_at(Meanwhile::Suspended, unprocessable, "validation_failed", json!(SUSPENDED))
        .await;
}

#[tokio::test]
async fn an_account_being_deleted_meanwhile_keeps_its_role() {
    promote_stops_at(Meanwhile::Deleting, StatusCode::NOT_FOUND, "not_found", Value::Null).await;
}

#[tokio::test]
async fn a_change_whose_request_is_dropped_mid_write_still_tells_the_account() {
    let (_temp, state, app) = accounts().await;
    let address = serve(app.clone()).await;
    let mut phone = connect_with(address, "phone-token").await;

    let (held, release) = hold_next(&state.db, "AFTER UPDATE OF role ON users").await;
    let request = tokio::spawn({
        let app = app.clone();
        async move { set(&app, TOKEN, 4, "admin", PASSWORD).await }
    });
    held.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    drop(release);
    assert_eq!(next_of(&mut phone, "accessChanged").await, json!({ "type": "accessChanged" }));
}

#[tokio::test]
async fn a_promotion_ends_the_reset_link_an_admin_issued_for_the_user() {
    let (_temp, _state, app) = accounts().await;
    let code = issue_reset(&app, "admin-token", 4).await;
    assert_eq!(check_reset(&app, &code).await, StatusCode::OK);

    let (status, account) = set(&app, TOKEN, 4, "admin", PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{account}");
    assert_eq!((&account["role"], &account["resetPending"]), (&json!("admin"), &json!(false)));
    reset_ended(&app, &code).await;
}

#[tokio::test]
async fn a_demotion_ends_the_reset_link_the_owner_issued_for_the_admin() {
    let (_temp, _state, app) = accounts().await;
    let code = issue_reset(&app, TOKEN, 3).await;
    assert_eq!(check_reset(&app, &code).await, StatusCode::OK);

    let (status, account) = set(&app, TOKEN, 3, "user", PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{account}");
    assert_eq!((&account["role"], &account["resetPending"]), (&json!("user"), &json!(false)));
    reset_ended(&app, &code).await;
}

#[tokio::test]
async fn a_promoted_user_reaches_every_library_and_a_demoted_one_only_their_grants() {
    let (temp, _state, app) = accounts().await;
    let conn = conn(&temp);
    assert_eq!(reached(&app, "user-token").await, [json!("1")]);
    assert_eq!(liked(&conn, 4), [0, 0]);

    let (status, account) = set(&app, TOKEN, 4, "admin", PASSWORD).await;
    assert_eq!((status, &account["libraryIds"]), (StatusCode::OK, &json!(["1"])));
    assert_eq!(reached(&app, "user-token").await, [json!("1"), json!("2")]);
    assert_eq!(liked(&conn, 4), [1, 1]);

    let (status, account) = set(&app, TOKEN, 4, "user", PASSWORD).await;
    assert_eq!((status, &account["libraryIds"]), (StatusCode::OK, &json!(["1"])));
    assert_eq!(reached(&app, "user-token").await, [json!("1")]);
}

#[tokio::test]
async fn a_demoted_admin_loses_the_admin_pages_and_their_pending_invites_at_once() {
    let (_temp, _state, app) = accounts().await;
    let revoked = invite(&app, "admin-token").await;
    let kept = invite(&app, "admin3-token").await;
    assert_eq!(
        call(&app, "admin-token", "GET", "/api/v1/admin/users", None).await.0,
        StatusCode::OK
    );

    assert_eq!(set(&app, TOKEN, 2, "user", PASSWORD).await.0, StatusCode::OK);
    let (status, problem) = call(&app, "admin-token", "GET", "/api/v1/admin/users", None).await;
    assert_eq!((status, &problem["code"]), (StatusCode::FORBIDDEN, &json!("forbidden")));
    assert_eq!(invite_status(&app, &revoked).await, "revoked");
    assert_eq!(invite_status(&app, &kept).await, "pending");
}

#[tokio::test]
async fn the_owners_own_role_a_suspended_accounts_and_owner_cannot_be_set() {
    let (temp, _state, app) = accounts_with_links().await;
    let conn = conn(&temp);
    conn.execute("UPDATE users SET status = 'suspended' WHERE id = 5", []).unwrap();
    let before = snapshot(&conn);

    let (status, problem) = set(&app, TOKEN, 1, "admin", PASSWORD).await;
    assert_eq!((status, &problem["code"]), (StatusCode::FORBIDDEN, &json!("forbidden")));
    assert_eq!(problem["detail"], "You cannot do this to your own account.");

    let (status, problem) = set(&app, TOKEN, 5, "admin", PASSWORD).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(
        (&problem["code"], &problem["detail"]),
        (&json!("validation_failed"), &json!(SUSPENDED))
    );

    let (status, problem) = set(&app, TOKEN, 4, "owner", PASSWORD).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "validation_failed");
    let detail = problem["detail"].as_str().unwrap();
    assert!(detail.contains("unknown variant `owner`"), "{detail}");

    assert_eq!(snapshot(&conn), before);
}

#[tokio::test]
async fn the_accounts_devices_alone_are_told() {
    let (_temp, _state, app) = accounts().await;
    let address = serve(app.clone()).await;
    let mut laptop = connect_with(address, "user-token").await;
    let mut phone = connect_with(address, "phone-token").await;
    let mut other = connect_with(address, "other-token").await;

    assert_eq!(set(&app, TOKEN, 4, "admin", PASSWORD).await.0, StatusCode::OK);
    for socket in [&mut laptop, &mut phone] {
        assert_eq!(next_of(socket, "accessChanged").await, json!({ "type": "accessChanged" }));
    }
    hears_no_change(&mut other).await;

    assert_eq!(set(&app, TOKEN, 4, "user", PASSWORD).await.0, StatusCode::OK);
    assert_eq!(next_of(&mut phone, "accessChanged").await, json!({ "type": "accessChanged" }));
}

#[tokio::test]
async fn each_change_is_recorded_and_the_role_already_held_changes_nothing() {
    let (temp, _state, app) = accounts().await;
    let conn = conn(&temp);
    let address = serve(app.clone()).await;
    let mut user = connect_with(address, "user-token").await;
    let mut other = connect_with(address, "other-token").await;

    assert_eq!(set(&app, TOKEN, 4, "admin", PASSWORD).await.0, StatusCode::OK);
    assert_eq!(set(&app, TOKEN, 4, "user", PASSWORD).await.0, StatusCode::OK);
    let changes: Vec<_> =
        audit(&conn).into_iter().filter(|(action, ..)| action == "role_changed").collect();
    assert_eq!(
        changes,
        [
            ("role_changed".to_owned(), 1, 4, json!({ "from": "user", "to": "admin" })),
            ("role_changed".to_owned(), 1, 4, json!({ "from": "admin", "to": "user" })),
        ]
    );
    next_of(&mut user, "accessChanged").await;
    next_of(&mut user, "accessChanged").await;

    // A reset link and an invite that a change would clear stay.
    issue_reset(&app, TOKEN, 5).await;
    issue_reset(&app, TOKEN, 3).await;
    invite(&app, "admin3-token").await;
    let before = snapshot(&conn);
    let (status, account) = set(&app, TOKEN, 5, "user", PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{account}");
    assert_eq!((&account["role"], &account["resetPending"]), (&json!("user"), &json!(true)));
    let (status, account) = set(&app, TOKEN, 3, "admin", PASSWORD).await;
    assert_eq!((status, &account["role"]), (StatusCode::OK, &json!("admin")));
    assert_eq!(snapshot(&conn), before);
    hears_no_change(&mut other).await;
    hears_no_change(&mut user).await;
}
