//! Deleting an account.

use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use futures_util::{SinkExt, StreamExt};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::api::password::{hash_new, hold_checks};
use crate::api::testing::player::{KEY, context, item, start_playing, start_queue};
use crate::api::testing::socket::{Socket, connect_with, next, next_of, serve};
use crate::api::testing::{
    TOKEN, add_device, add_user, app_with_tracks, call, conn, eventually, hold_next, router_over,
    send_json, state,
};
use crate::api::{AppState, Uuid};
use crate::db::testing::{User, access, mark_deleting};
use crate::queue::NewQueue;

const PASSWORD: &str = "correct horse battery staple";

/// The owner (user 1, [`TOKEN`]) over [`app_with_tracks`]; admins 2 (`admin-token`) and 3
/// (`admin3-token`); user 4 reaching library 1, signed in on devices 4 (`user-token`) and 7
/// (`phone-token`); user 5 (`other-token`), each of them with [`PASSWORD`]; and user 6, being
/// deleted. Answers the state too, to look at what it holds in memory.
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

/// `adminDeleteUser` on account `user` as `token`, confirmed with `password`.
async fn delete(app: &Router, token: &str, user: i64, password: &str) -> (StatusCode, Value) {
    let uri = format!("/api/v1/admin/users/{user}/delete");
    call(app, token, "POST", &uri, Some(json!({ "password": password }))).await
}

/// Logs in as `username` with `password`, and returns the status and JSON body.
async fn login(app: &Router, username: &str, password: &str) -> (StatusCode, Value) {
    let body = json!({
        "username": username,
        "password": password,
        "device": { "name": "Firefox on Linux", "type": "desktop" }
    });
    let (status, _, body) = send_json(app.clone(), "POST", "/api/v1/auth/login", &body).await;
    (status, body)
}

/// A pending invite to library 1 made by `token`'s account, and its ID and code.
async fn invite(app: &Router, token: &str) -> (String, String) {
    let body = json!({ "libraryIds": ["1"] });
    let (status, created) = call(app, token, "POST", "/api/v1/admin/invites", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    (created["id"].as_str().unwrap().to_owned(), created["code"].as_str().unwrap().to_owned())
}

/// The status of invite `id` as the admins' list shows it.
async fn invite_status(app: &Router, id: &str) -> Value {
    let (_, page) = call(app, TOKEN, "GET", "/api/v1/admin/invites", None).await;
    let items = page["items"].as_array().unwrap();
    items.iter().find(|item| item["id"] == id).unwrap()["status"].clone()
}

/// Redeems invite `code` as a new account named `username`, and answers the status.
async fn redeem(app: &Router, code: &str, username: &str) -> StatusCode {
    let body = json!({
        "code": code,
        "username": username,
        "password": "vqp-mfrk-ztlw",
        "device": { "name": "Firefox on Linux", "type": "desktop" }
    });
    send_json(app.clone(), "POST", "/api/v1/auth/invites/redeem", &body).await.0
}

/// Account `user`'s status, or `None` once its row is gone.
fn account_status(conn: &Connection, user: i64) -> Option<String> {
    conn.query_row("SELECT status FROM users WHERE id = ?1", [user], |row| row.get(0))
        .optional()
        .unwrap()
}

/// Waits until account `user`'s row is gone, which the deletion task removes last.
async fn emptied(conn: &Connection, user: i64) {
    eventually("the account is emptied", async || {
        account_status(conn, user).is_none().then_some(())
    })
    .await;
}

/// Account `user`'s devices, each with why it was signed out.
fn devices(conn: &Connection, user: i64) -> Vec<(i64, Option<String>)> {
    conn.prepare("SELECT id, revoked_reason FROM devices WHERE user_id = ?1 ORDER BY id")
        .unwrap()
        .query_map([user], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// The `deleted` rows of the audit log, as (actor, actor's name, target, target's name, detail).
fn deletions(conn: &Connection) -> Vec<(i64, String, i64, String, Value)> {
    conn.prepare(
        "SELECT actor_id, actor_name, target_id, target_name, detail FROM admin_actions \
         WHERE action = 'deleted' ORDER BY id",
    )
    .unwrap()
    .query_map([], |row| {
        let detail: String = row.get(4)?;
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            serde_json::from_str(&detail).unwrap(),
        ))
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// How many sign-in attempts are recorded, password confirmations among them.
fn attempts(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM login_attempts", [], |row| row.get(0)).unwrap()
}

/// Asserts that `socket` is told nothing of being signed out before it answers a ping.
async fn stays_signed_in(socket: &mut Socket) {
    socket.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    loop {
        let message = next(socket).await;
        assert_ne!(message["type"], "sessionRevoked", "{message}");
        if message["type"] == "pong" {
            return;
        }
    }
}

/// Asserts that `socket` is told it was signed out with `logged_out`, then closed.
async fn signed_out(socket: &mut Socket) {
    let told = next_of(socket, "sessionRevoked").await;
    assert_eq!(told, json!({ "type": "sessionRevoked", "reason": "logged_out" }));
    let after = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap();
    assert!(matches!(after, None | Some(Ok(Message::Close(_)))), "{after:?}");
}

#[tokio::test]
async fn a_deleted_account_is_gone_at_once_and_its_name_is_free() {
    let (_temp, _state, app) = accounts().await;
    let (revoked, _) = invite(&app, "admin3-token").await;
    let (kept, code) = invite(&app, "admin-token").await;
    let address = serve(app.clone()).await;
    let mut laptop = connect_with(address, "admin3-token").await;
    let mut other = connect_with(address, "admin-token").await;

    assert_eq!(delete(&app, TOKEN, 3, PASSWORD).await, (StatusCode::NO_CONTENT, Value::Null));

    let unknown = login(&app, "nobody", PASSWORD).await;
    assert_eq!(unknown.0, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.1["code"], "invalid_credentials");
    assert_eq!(login(&app, "user3", PASSWORD).await, unknown);
    let no_token = call(&app, "no-such-token", "GET", "/api/v1/me", None).await;
    assert_eq!(no_token.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, "admin3-token", "GET", "/api/v1/me", None).await, no_token);

    signed_out(&mut laptop).await;
    stays_signed_in(&mut other).await;

    let (_, page) = call(&app, TOKEN, "GET", "/api/v1/admin/users", None).await;
    let listed: Vec<&Value> = page["items"].as_array().unwrap().iter().map(|a| &a["id"]).collect();
    assert_eq!(listed, [&json!("1"), &json!("2"), &json!("4"), &json!("5")]);
    let (_, log) = call(&app, TOKEN, "GET", "/api/v1/admin/audit-log", None).await;
    let entry = &log["items"][0];
    assert_eq!(entry["action"], "deleted");
    assert_eq!(entry["target"], json!({ "id": "3", "username": "user3", "deleted": true }));
    assert_eq!(entry["actor"], json!({ "id": "1", "username": "owner", "deleted": false }));

    assert_eq!(invite_status(&app, &revoked).await, "revoked");
    assert_eq!(invite_status(&app, &kept).await, "pending");
    assert_eq!(redeem(&app, &code, "user3").await, StatusCode::CREATED);
}

/// Deletion revokes its creator's pending invites, so this one is left pending by hand.
#[tokio::test]
async fn an_invite_whose_creator_is_gone_can_still_be_redeemed_with_no_actor() {
    let (temp, _state, app) = accounts().await;
    let conn = conn(&temp);
    let (id, code) = invite(&app, "admin3-token").await;
    assert_eq!(delete(&app, TOKEN, 3, PASSWORD).await.0, StatusCode::NO_CONTENT);
    emptied(&conn, 3).await;
    conn.execute("UPDATE invites SET revoked_at = NULL WHERE id = ?1", [&id]).unwrap();
    let creator: Option<i64> = conn
        .query_row("SELECT created_by FROM invites WHERE id = ?1", [&id], |row| row.get(0))
        .unwrap();
    assert_eq!(creator, None);

    assert_eq!(redeem(&app, &code, "newcomer").await, StatusCode::CREATED);
    let created: (Option<i64>, Option<String>, String, Value) = conn
        .query_row(
            "SELECT actor_id, actor_name, target_name, detail FROM admin_actions \
             WHERE action = 'account_created'",
            [],
            |row| {
                let detail: String = row.get(3)?;
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, serde_json::from_str(&detail).unwrap()))
            },
        )
        .unwrap();
    assert_eq!(
        created,
        (None, None, "newcomer".to_owned(), json!({ "inviteId": id, "libraryIds": ["1"] }))
    );
}

/// The task is held at its first device, so what the request itself did to the account shows.
#[tokio::test]
async fn the_request_clears_the_name_and_reset_link_and_leaves_the_devices_to_the_task() {
    let (temp, state, app) = accounts().await;
    let conn = conn(&temp);
    let uri = "/api/v1/admin/users/4/password-reset";
    let issued = call(&app, TOKEN, "POST", uri, Some(json!({ "password": PASSWORD }))).await;
    assert_eq!(issued.0, StatusCode::CREATED, "{}", issued.1);
    let (held, release) = hold_next(&state.db, "BEFORE DELETE ON devices").await;

    let answered = tokio::time::timeout(Duration::from_secs(30), delete(&app, TOKEN, 4, PASSWORD));
    let answered = answered.await.expect("the request's write deletes no device");
    assert_eq!(answered.0, StatusCode::NO_CONTENT);
    held.await.unwrap();
    let account: (String, Option<String>, Option<Vec<u8>>, Option<i64>) = conn
        .query_row(
            "SELECT status, username, reset_hash, reset_expires_at FROM users WHERE id = 4",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(account, ("deleting".to_owned(), None, None, None));
    assert_eq!(devices(&conn, 4), [(4, None), (7, None)]);
    drop(release);
    emptied(&conn, 4).await;
    assert!(devices(&conn, 4).is_empty());
}

/// Every row of `user` in a table with a `user_id` column, by table.
fn rows_of(conn: &Connection, user: i64) -> Vec<(String, i64)> {
    let tables: Vec<String> = conn
        .prepare(
            "SELECT m.name FROM sqlite_schema m JOIN pragma_table_info(m.name) c \
             WHERE m.type = 'table' AND c.name = 'user_id' ORDER BY m.name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    tables
        .into_iter()
        .map(|table| {
            let sql = format!("SELECT count(*) FROM {table} WHERE user_id = ?1");
            let count = conn.query_row(&sql, [user], |row| row.get(0)).unwrap();
            (table, count)
        })
        .filter(|(_, count)| *count > 0)
        .collect()
}

/// No sweep runs in these tests: only the request's own kick can empty the account.
#[tokio::test]
async fn the_account_is_emptied_without_waiting_for_the_sweep() {
    let (temp, _state, app) = accounts().await;
    let conn = conn(&temp);
    assert_eq!(login(&app, "user4", PASSWORD).await.0, StatusCode::OK);
    let before = rows_of(&conn, 4);
    for table in ["devices", "library_access", "login_attempts"] {
        assert!(before.iter().any(|(name, _)| name == table), "{table}: {before:?}");
    }
    let others = rows_of(&conn, 5);

    assert_eq!(delete(&app, "admin-token", 4, PASSWORD).await.0, StatusCode::NO_CONTENT);
    emptied(&conn, 4).await;
    assert!(rows_of(&conn, 4).is_empty());
    assert_eq!(rows_of(&conn, 5), others);
}

#[tokio::test]
async fn deleting_obeys_the_rank_rule() {
    let (temp, _state, app) = accounts().await;
    let conn = conn(&temp);
    let self_ = "You cannot do this to your own account.";
    let owner = "No one can do this to the owner's account.";
    let admin = "Only the owner can do this to an admin's account.";
    for (token, user, detail) in [
        ("admin-token", 3, admin),
        ("admin-token", 1, owner),
        ("admin-token", 2, self_),
        (TOKEN, 1, self_),
    ] {
        let (status, problem) = delete(&app, token, user, PASSWORD).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{token} on {user}: {problem}");
        assert_eq!(problem["code"], "forbidden", "{token} on {user}");
        assert_eq!(problem["detail"], detail, "{token} on {user}");
    }
    for user in [99, 6] {
        let (status, problem) = delete(&app, TOKEN, user, PASSWORD).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{user}: {problem}");
        assert_eq!(
            problem,
            json!({ "type": "about:blank", "title": "Not Found", "status": 404, "code": "not_found" })
        );
    }
    let (status, problem) = delete(&app, "other-token", 4, PASSWORD).await;
    assert_eq!((status, &problem["code"]), (StatusCode::FORBIDDEN, &json!("forbidden")));

    let (status, problem) = delete(&app, "admin-token", 4, "wrong horse").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    assert_eq!(problem["code"], "wrong_password");
    assert_eq!(problem["detail"], "Your current password is not correct.");

    for user in [1, 2, 3, 4, 5] {
        assert_eq!(account_status(&conn, user).as_deref(), Some("active"), "{user}");
    }
    assert!(deletions(&conn).is_empty());
}

#[tokio::test]
async fn the_record_names_the_account_after_it_is_gone_and_its_name_retaken() {
    let (temp, _state, app) = accounts().await;
    let conn = conn(&temp);
    let (_, code) = invite(&app, "admin3-token").await;
    assert_eq!(delete(&app, "admin-token", 4, PASSWORD).await.0, StatusCode::NO_CONTENT);
    assert_eq!(deletions(&conn), [(2, "user2".to_owned(), 4, "user4".to_owned(), json!({}))]);

    emptied(&conn, 4).await;
    assert_eq!(redeem(&app, &code, "user4").await, StatusCode::CREATED);
    let (_, log) = call(&app, TOKEN, "GET", "/api/v1/admin/audit-log", None).await;
    let deleted: Vec<&Value> = log["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["action"] == "deleted")
        .collect();
    assert_eq!(deleted.len(), 1, "{log}");
    assert_eq!(deleted[0]["target"], json!({ "id": "4", "username": "user4", "deleted": true }));
}

#[tokio::test]
async fn a_deletion_whose_request_is_dropped_mid_write_still_closes_the_sockets_and_empties_it() {
    let (temp, state, app) = accounts().await;
    let conn = conn(&temp);
    let address = serve(app.clone()).await;
    let mut phone = connect_with(address, "phone-token").await;

    let (held, release) = hold_next(&state.db, "AFTER UPDATE OF status ON users").await;
    let request = tokio::spawn({
        let app = app.clone();
        async move { delete(&app, TOKEN, 4, PASSWORD).await }
    });
    held.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    drop(release);
    signed_out(&mut phone).await;
    emptied(&conn, 4).await;
}

/// What happens between the request's password check and its write.
enum Meanwhile {
    /// Admin 2, deleting, is suspended.
    Suspended,
    /// Admin 2, deleting, becomes a user.
    Demoted,
    /// Admin 2's password changes.
    PasswordChanged,
    /// User 4, being deleted, becomes an admin.
    Promoted,
}

/// Deletes user 4 as admin 2 with their password check held while `meanwhile` happens, and
/// asserts that the request is refused with `status`, `code` and `detail`, and changes nothing.
async fn delete_stops_at(meanwhile: Meanwhile, status: StatusCode, code: &str, detail: Value) {
    let (temp, _state, app) = accounts().await;
    let conn = conn(&temp);
    let elsewhere = hash_new("vqp-mfrk-ztlw".to_owned(), Vec::new()).await.unwrap();
    let before = attempts(&conn);

    let held = hold_checks().await;
    let racing = tokio::spawn({
        let app = app.clone();
        async move { delete(&app, "admin-token", 4, PASSWORD).await }
    });
    eventually("the attempt is recorded", async || (attempts(&conn) > before).then_some(())).await;
    match meanwhile {
        Meanwhile::Suspended => {
            conn.execute("UPDATE users SET status = 'suspended' WHERE id = 2", [])
        }
        Meanwhile::Demoted => conn.execute("UPDATE users SET role = 'user' WHERE id = 2", []),
        Meanwhile::PasswordChanged => {
            conn.execute("UPDATE users SET password = ?1 WHERE id = 2", [&elsewhere])
        }
        Meanwhile::Promoted => conn.execute("UPDATE users SET role = 'admin' WHERE id = 4", []),
    }
    .unwrap();
    drop(held);

    let (answered, problem) = racing.await.unwrap();
    assert_eq!((answered, &problem["code"]), (status, &json!(code)), "{problem}");
    assert_eq!(problem.get("detail").cloned().unwrap_or(Value::Null), detail);
    assert_eq!(account_status(&conn, 4).as_deref(), Some("active"));
    assert!(deletions(&conn).is_empty());
}

/// Answered as the gate would now answer the suspended admin's token, as `Confirmed::check`
/// does for every password-confirmed action.
#[tokio::test]
async fn an_admin_suspended_while_deleting_deletes_nothing() {
    let unauthenticated = StatusCode::UNAUTHORIZED;
    delete_stops_at(Meanwhile::Suspended, unauthenticated, "unauthenticated", Value::Null).await;
}

#[tokio::test]
async fn an_admin_demoted_while_deleting_deletes_nothing() {
    let detail = json!("Only an admin or the owner can manage accounts.");
    delete_stops_at(Meanwhile::Demoted, StatusCode::FORBIDDEN, "forbidden", detail).await;
}

#[tokio::test]
async fn an_admin_whose_password_changed_while_deleting_deletes_nothing() {
    let detail = json!("Your current password is not correct.");
    delete_stops_at(Meanwhile::PasswordChanged, StatusCode::FORBIDDEN, "wrong_password", detail)
        .await;
}

#[tokio::test]
async fn an_account_promoted_while_an_admin_deletes_it_is_kept() {
    let detail = json!("Only the owner can do this to an admin's account.");
    delete_stops_at(Meanwhile::Promoted, StatusCode::FORBIDDEN, "forbidden", detail).await;
}

/// Both pass their password checks before either writes, so the second is stopped by its write
/// alone.
#[tokio::test]
async fn of_two_deletions_at_once_one_applies() {
    let (temp, _state, app) = accounts().await;
    let conn = conn(&temp);
    let before = attempts(&conn);

    let held = hold_checks().await;
    let deletions_at_once = ["admin-token", "admin3-token"].map(|token| {
        let app = app.clone();
        tokio::spawn(async move { delete(&app, token, 4, PASSWORD).await })
    });
    eventually("both attempts are recorded", async || {
        (attempts(&conn) >= before + 2).then_some(())
    })
    .await;
    drop(held);

    let mut answers = Vec::new();
    for deletion in deletions_at_once {
        answers.push(deletion.await.unwrap());
    }
    answers.sort_by_key(|(status, _)| *status);
    assert_eq!(answers[0], (StatusCode::NO_CONTENT, Value::Null));
    assert_eq!(
        answers[1],
        (
            StatusCode::NOT_FOUND,
            json!({ "type": "about:blank", "title": "Not Found", "status": 404, "code": "not_found" })
        )
    );
    assert_eq!(deletions(&conn).len(), 1);
}

/// The deleted account's own password change, past its check when the deletion commits, is
/// refused by its write. A play or other row written late goes with the task's last cascade,
/// or fails on its key once the account's row is gone
/// (`db::retention::accounts::tests::rows_written_late_go_with_the_account_row_and_after_it_fail_alone`).
#[tokio::test]
async fn the_deleted_accounts_own_password_change_in_flight_changes_nothing() {
    let (temp, _state, app) = accounts().await;
    let conn = conn(&temp);
    let before = attempts(&conn);
    let password: String =
        conn.query_row("SELECT password FROM users WHERE id = 4", [], |row| row.get(0)).unwrap();

    let held = hold_checks().await;
    let racing = tokio::spawn({
        let app = app.clone();
        async move {
            let body = json!({ "currentPassword": PASSWORD, "newPassword": "vqp-mfrk-ztlw" });
            call(&app, "user-token", "PUT", "/api/v1/me/password", Some(body)).await
        }
    });
    eventually("the attempt is recorded", async || (attempts(&conn) > before).then_some(())).await;
    // The deletion's write, as the request makes it.
    mark_deleting(&conn, 4).unwrap();
    drop(held);

    let (answered, problem) = racing.await.unwrap();
    assert_eq!((answered, &problem["code"]), (StatusCode::UNAUTHORIZED, &json!("unauthenticated")));
    let after: String =
        conn.query_row("SELECT password FROM users WHERE id = 4", [], |row| row.get(0)).unwrap();
    assert_eq!(after, password);
    assert_eq!(devices(&conn, 4), [(4, None), (7, None)]);
}

/// A queue for user 4 of library 1's tracks 1 and 2, as `startQueue` makes one.
fn new_queue() -> NewQueue {
    NewQueue {
        queue_id: Uuid::try_from("6f0c6a52-3b7e-4d43-9a8e-0d1c2b3a4f5e".to_owned()).unwrap(),
        context: context(),
        key: KEY.to_owned(),
        library_id: 1,
        tracks: [1, 2].into(),
        recipe: None,
        at: 0,
    }
}

/// Neither the player's nor the queues' memory keeps what the account held when the request
/// came, and what a request already past the gate gives it once the account's row is gone is
/// dropped at the first save, so neither tries to save it again.
#[tokio::test]
async fn nothing_in_memory_is_kept_for_a_deleted_account() {
    let (temp, state, app) = accounts().await;
    let conn = conn(&temp);
    start_queue(&app, "user-token").await;
    let address = serve(app.clone()).await;
    let mut playing = connect_with(address, "user-token").await;
    start_playing(&mut playing).await;
    assert!(state.players.position(4).is_some());
    assert!(state.queues.holds(4, &item(0)));

    let (held, release) = hold_next(&state.db, "BEFORE DELETE ON devices").await;
    assert_eq!(delete(&app, TOKEN, 4, PASSWORD).await.0, StatusCode::NO_CONTENT);
    held.await.unwrap();
    assert!(state.players.position(4).is_none());
    assert!(!state.queues.holds(4, &item(0)));

    drop(release);
    emptied(&conn, 4).await;

    // A start of a request already past the gate, which reaches the writer only once the task
    // has finished, puts the queue back in memory; the first save that finds the account gone
    // drops it, rather than trying it again every save.
    state.queues.start(&state.db, 4, new_queue(), || None).await.unwrap();
    assert!(state.queues.holds(4, &item(0)));
    state.queues.save(&state.db).await;
    assert!(!state.queues.holds(4, &item(0)));
    state.players.save(&state.db).await;
    assert!(rows_of(&conn, 4).is_empty());
}
