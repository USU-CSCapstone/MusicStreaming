//! Changing one's own password.

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use futures_util::{SinkExt, StreamExt};
use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

use crate::api::password::{hash_new, hold_checks};
use crate::api::session::token_hash;
use crate::api::testing::socket::{Socket, connect_with, next, next_of, serve};
use crate::api::testing::{
    TOKEN, add_device, add_signed_out_device, app, call, conn, eventually, send_json,
};
use crate::db::testing::User;
use crate::db::{Database, now_ms};

const PASSWORD: &str = "correct horse battery staple";
const NEW: &str = "vqp-mfrk-ztlw";
const OTHER_NEW: &str = "Ὀδυσσεὺς ἐν Ἰθάκῃ ἦν";

/// The owner (user 1, [`TOKEN`]); user 2 with [`PASSWORD`], signed in on devices 2
/// (`user-token`), 3 (`phone-token`) and 4 (`tablet-token`) and signed out of device 5; and
/// user 3 with [`PASSWORD`], signed in on device 6 (`other-token`).
async fn accounts() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app("");
    let conn = conn(&temp);
    let password = hash_new(PASSWORD.to_owned(), Vec::new()).await.unwrap();
    for id in [2, 3] {
        User { password: password.clone(), ..User::new(id, "user") }.insert(&conn).unwrap();
    }
    add_device(&conn, 2, 2, "user-token");
    add_device(&conn, 2, 3, "phone-token");
    add_device(&conn, 2, 4, "tablet-token");
    add_signed_out_device(&conn, 2, 5, "old-token", "logged_out");
    add_device(&conn, 3, 6, "other-token");
    (temp, db, app)
}

/// `changeMyPassword` as `token`.
async fn change(app: &Router, token: &str, current: &str, new: &str) -> (StatusCode, Value) {
    let body = json!({ "currentPassword": current, "newPassword": new });
    call(app, token, "PUT", "/api/v1/me/password", Some(body)).await
}

/// The status of logging in as `username` with `password`.
async fn login(app: &Router, username: &str, password: &str) -> StatusCode {
    let body = json!({
        "username": username,
        "password": password,
        "device": { "name": "Firefox on Linux", "type": "desktop" }
    });
    send_json(app.clone(), "POST", "/api/v1/auth/login", &body).await.0
}

/// Every device with why it was signed out.
fn revoked(conn: &Connection) -> Vec<(i64, Option<String>)> {
    conn.prepare("SELECT id, revoked_reason FROM devices ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// The devices of [`accounts`] as it makes them.
fn as_made() -> Vec<(i64, Option<String>)> {
    vec![(1, None), (2, None), (3, None), (4, None), (5, Some("logged_out".into())), (6, None)]
}

/// Account `user`'s stored password hash.
fn stored(conn: &Connection, user: i64) -> String {
    conn.query_row("SELECT password FROM users WHERE id = ?1", [user], |row| row.get(0)).unwrap()
}

/// How many sign-in attempts are recorded, password confirmations among them.
fn attempts(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM login_attempts", [], |row| row.get(0)).unwrap()
}

/// Moves every recorded attempt so that the latest was made `ms` ago, so a wait that runs from
/// it is not over by the time a slow machine makes the next attempt.
fn since_latest(conn: &Connection, ms: i64) {
    conn.execute(
        "UPDATE login_attempts \
         SET attempted_at = attempted_at + ?1 - (SELECT max(attempted_at) FROM login_attempts)",
        [now_ms() - ms],
    )
    .unwrap();
}

/// The device IDs in a `devices` message, in its order.
fn ids(message: &Value) -> Vec<Value> {
    message["devices"].as_array().unwrap().iter().map(|d| d["id"].clone()).collect()
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

/// Asserts that `socket` is told it was signed out with `credentials_changed`, then closed.
async fn signed_out(socket: &mut Socket) {
    let told = next_of(socket, "sessionRevoked").await;
    assert_eq!(told, json!({ "type": "sessionRevoked", "reason": "credentials_changed" }));
    let after = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap();
    assert!(matches!(after, None | Some(Ok(Message::Close(_)))), "{after:?}");
}

#[tokio::test]
async fn a_change_sets_the_new_password_and_keeps_the_calling_device_signed_in() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let others = stored(&conn, 3);

    let (status, body) = change(&app, "user-token", PASSWORD, NEW).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));

    let mut expected = as_made();
    expected[2].1 = Some("credentials_changed".into());
    expected[3].1 = Some("credentials_changed".into());
    assert_eq!(revoked(&conn), expected, "only the caller's other devices");
    assert_eq!(stored(&conn, 3), others);
    assert_eq!(call(&app, "user-token", "GET", "/api/v1/me", None).await.0, StatusCode::OK);
    let (status, problem) = call(&app, "phone-token", "GET", "/api/v1/me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        (&problem["code"], &problem["reason"]),
        (&json!("session_revoked"), &json!("credentials_changed"))
    );
    assert_eq!(call(&app, "other-token", "GET", "/api/v1/me", None).await.0, StatusCode::OK);
    assert_eq!(login(&app, "user2", PASSWORD).await, StatusCode::UNAUTHORIZED);
    assert_eq!(login(&app, "user2", NEW).await, StatusCode::OK);
    assert_eq!(login(&app, "user3", PASSWORD).await, StatusCode::OK);
}

#[tokio::test]
async fn the_other_devices_sockets_close_and_the_callers_stays_open() {
    let (_temp, _db, app) = accounts().await;
    let address = serve(app.clone()).await;
    let mut caller = connect_with(address, "user-token").await;
    let mut phone = connect_with(address, "phone-token").await;
    let mut tablet = connect_with(address, "tablet-token").await;
    let mut other = connect_with(address, "other-token").await;

    assert_eq!(change(&app, "user-token", PASSWORD, NEW).await.0, StatusCode::NO_CONTENT);

    signed_out(&mut phone).await;
    signed_out(&mut tablet).await;
    stays_signed_in(&mut caller).await;
    stays_signed_in(&mut other).await;
}

#[tokio::test]
async fn the_remaining_devices_are_sent_the_new_list() {
    let (_temp, _db, app) = accounts().await;
    let address = serve(app.clone()).await;
    // No device about to be signed out holds a socket, so a list without them can come only
    // from the change, not from a closing connection.
    let mut caller = connect_with(address, "user-token").await;
    let first = next_of(&mut caller, "devices").await;
    assert!(ids(&first).contains(&json!("3")), "{first}");

    assert_eq!(change(&app, "user-token", PASSWORD, NEW).await.0, StatusCode::NO_CONTENT);

    loop {
        let list = ids(&next_of(&mut caller, "devices").await);
        if !list.contains(&json!("3")) {
            assert_eq!(list, [json!("2")]);
            break;
        }
    }
}

/// Holds the writer inside the next write that changes a password, until the sender is
/// dropped; the receiver hears when it is held.
async fn hold_next_change(db: &Database) -> (oneshot::Receiver<()>, mpsc::Sender<()>) {
    let (entered, held) = oneshot::channel();
    let (release, released) = mpsc::channel();
    let hold = Mutex::new(Some((entered, released)));
    db.write(move |tx| {
        tx.create_scalar_function("hold", 0, FunctionFlags::SQLITE_UTF8, move |_| {
            if let Some((entered, released)) = hold.lock().unwrap().take() {
                entered.send(()).unwrap();
                let _ = released.recv();
            }
            Ok(0)
        })?;
        tx.execute_batch(
            "CREATE TEMP TRIGGER IF NOT EXISTS hold AFTER UPDATE OF password ON users \
             BEGIN SELECT hold(); END",
        )
    })
    .await
    .unwrap();
    (held, release)
}

#[tokio::test]
async fn a_change_whose_request_is_dropped_mid_write_still_closes_the_other_sockets() {
    let (_temp, db, app) = accounts().await;
    let address = serve(app.clone()).await;
    let mut phone = connect_with(address, "phone-token").await;

    let (held, release) = hold_next_change(&db).await;
    let request = tokio::spawn({
        let app = app.clone();
        async move { change(&app, "user-token", PASSWORD, NEW).await }
    });
    held.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    drop(release);
    signed_out(&mut phone).await;
}

/// What happens between the caller's password being checked and the change's write.
enum Meanwhile {
    PasswordChanged,
    Suspended,
    DeviceSignedOut,
}

/// Changes user 2's password from `user-token` with its password check held while `meanwhile`
/// happens, and asserts that the change is refused with `code`, and `reason` if it has one, and
/// changes nothing.
async fn stops_at(meanwhile: Meanwhile, code: &str, reason: Option<&str>) {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let elsewhere = hash_new(OTHER_NEW.to_owned(), Vec::new()).await.unwrap();
    let before = attempts(&conn);

    // Checks wait, so the request stops at its password check, its attempt recorded and the
    // stored hash it checks against read.
    let held = hold_checks().await;
    let racing = tokio::spawn({
        let app = app.clone();
        async move { change(&app, "user-token", PASSWORD, NEW).await }
    });
    eventually("the attempt is recorded", async || (attempts(&conn) > before).then_some(())).await;
    match meanwhile {
        Meanwhile::PasswordChanged => {
            conn.execute("UPDATE users SET password = ?1 WHERE id = 2", [&elsewhere]).unwrap();
        }
        Meanwhile::Suspended => {
            let body = json!({ "status": "suspended" });
            let uri = "/api/v1/admin/users/2/status";
            assert_eq!(call(&app, TOKEN, "PUT", uri, Some(body)).await.0, StatusCode::OK);
        }
        Meanwhile::DeviceSignedOut => {
            let status = call(&app, "phone-token", "DELETE", "/api/v1/me/devices/2", None).await.0;
            assert_eq!(status, StatusCode::NO_CONTENT);
        }
    }
    let hash = stored(&conn, 2);
    let devices = revoked(&conn);
    drop(held);

    let (status, problem) = racing.await.unwrap();
    assert_eq!(problem["code"], code, "{problem}");
    let expected = if code == "wrong_password" { 403 } else { 401 };
    assert_eq!(status.as_u16(), expected, "{problem}");
    assert_eq!(problem.get("reason").and_then(Value::as_str), reason, "{problem}");
    assert_eq!(stored(&conn, 2), hash, "the password is unchanged");
    assert_eq!(revoked(&conn), devices, "no device is signed out");
}

#[tokio::test]
async fn a_change_checked_before_the_password_changed_is_refused() {
    stops_at(Meanwhile::PasswordChanged, "wrong_password", None).await;
}

#[tokio::test]
async fn a_change_checked_before_a_suspension_is_refused_as_the_gate_would() {
    stops_at(Meanwhile::Suspended, "unauthenticated", None).await;
}

#[tokio::test]
async fn a_change_checked_before_its_device_was_signed_out_is_refused_as_the_gate_would() {
    stops_at(Meanwhile::DeviceSignedOut, "session_revoked", Some("logged_out")).await;
}

#[tokio::test]
async fn a_wrong_current_password_is_refused_and_counts_as_a_failed_sign_in() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let hash = stored(&conn, 2);
    for _ in 0..5 {
        let (status, problem) = change(&app, "user-token", "wrong horse", NEW).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
        assert_eq!(problem["code"], "wrong_password");
        assert_eq!(problem["detail"], "Your current password is not correct.");
    }
    since_latest(&conn, 0);
    // Refused without checking, so even the right password waits.
    let (status, problem) = change(&app, "user-token", PASSWORD, NEW).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{problem}");
    assert_eq!(problem["code"], "rate_limited");
    assert_eq!(problem["detail"], "Too many wrong passwords. Wait a moment and try again.");
    // The same limit as signing in.
    assert_eq!(login(&app, "user2", PASSWORD).await, StatusCode::TOO_MANY_REQUESTS);

    assert_eq!(stored(&conn, 2), hash);
    assert_eq!(revoked(&conn), as_made());
    assert_eq!(call(&app, "user-token", "GET", "/api/v1/me", None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_right_current_password_clears_the_accounts_failures() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    for _ in 0..2 {
        for _ in 0..4 {
            let status = change(&app, "user-token", "wrong horse", NEW).await.0;
            assert_eq!(status, StatusCode::FORBIDDEN);
        }
        since_latest(&conn, 0);
        // Weak, so the change goes no further than the confirmation.
        let (status, problem) = change(&app, "user-token", PASSWORD, "password").await;
        assert_eq!(
            (status, &problem["code"]),
            (StatusCode::UNPROCESSABLE_ENTITY, &json!("weak_password"))
        );
    }
}

#[tokio::test]
async fn a_weak_new_password_changes_nothing() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let (status, problem) = change(&app, "user-token", PASSWORD, "password").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "weak_password");
    assert_eq!(
        problem["detail"],
        "This is a top-10 common password. Choose a longer or less predictable password."
    );
    assert_eq!(revoked(&conn), as_made());
    assert_eq!(login(&app, "user2", PASSWORD).await, StatusCode::OK);
}

/// Strong alone, the password is weak once it is the account's own display name.
#[tokio::test]
async fn a_new_password_made_of_the_accounts_name_is_weak() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    conn.execute("UPDATE users SET display_name = 'vqp-mfrk-ztlw' WHERE id = 2", []).unwrap();
    let (status, problem) = change(&app, "user-token", PASSWORD, "vqp-mfrk-ztlw").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "weak_password");
    assert_eq!(login(&app, "user2", PASSWORD).await, StatusCode::OK);
}

#[tokio::test]
async fn of_two_changes_at_once_one_applies() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let before = attempts(&conn);

    // Both pass their password check against the same stored hash before either writes.
    let held = hold_checks().await;
    let changes = [NEW, OTHER_NEW].map(|new| {
        let app = app.clone();
        tokio::spawn(async move { change(&app, "user-token", PASSWORD, new).await })
    });
    eventually("both attempts are recorded", async || {
        (attempts(&conn) >= before + 2).then_some(())
    })
    .await;
    drop(held);

    let mut answers = Vec::new();
    for change in changes {
        let (status, problem) = change.await.unwrap();
        answers.push((status, problem["code"].clone()));
    }
    answers.sort_by_key(|(status, _)| *status);
    assert_eq!(
        answers,
        [(StatusCode::NO_CONTENT, Value::Null), (StatusCode::FORBIDDEN, json!("wrong_password"))]
    );
    let signs_in = [login(&app, "user2", NEW).await, login(&app, "user2", OTHER_NEW).await];
    assert_eq!(signs_in.iter().filter(|status| **status == StatusCode::OK).count(), 1);
}

#[tokio::test]
async fn a_change_clears_a_pending_reset() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    conn.execute(
        "UPDATE users SET reset_hash = ?1, reset_expires_at = ?2 WHERE id = 2",
        rusqlite::params![token_hash("reset").to_vec(), now_ms() + 60_000],
    )
    .unwrap();

    assert_eq!(change(&app, "user-token", PASSWORD, NEW).await.0, StatusCode::NO_CONTENT);

    let reset: (Option<Vec<u8>>, Option<i64>) = conn
        .query_row("SELECT reset_hash, reset_expires_at FROM users WHERE id = 2", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(reset, (None, None));
}
