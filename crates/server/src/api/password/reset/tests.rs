//! Issuing, checking and redeeming password reset links.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use futures_util::{SinkExt, StreamExt};
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::LIFETIME_MS;
use crate::api::password::{asked_to_hash, hash_new, hold_checks, verify};
use crate::api::session::token_hash;
use crate::api::testing::socket::{Socket, connect_with, next, next_of, serve};
use crate::api::testing::{
    TOKEN, add_device, add_user, app, app_before_setup, audit, call, conn, eventually,
    hold_next_password_write, respond,
};
use crate::api::time::rfc3339;
use crate::db::testing::{User, mark_deleting};
use crate::db::{Database, now_ms};

const CHECK: &str = "/api/v1/auth/password-resets/check";
const REDEEM: &str = "/api/v1/auth/password-resets/redeem";
const PASSWORD: &str = "correct horse battery staple";
const NEW: &str = "vqp-mfrk-ztlw";
const OTHER_NEW: &str = "Ὀδυσσεὺς ἐν Ἰθάκῃ ἦν";

/// The owner (user 1, [`TOKEN`]), admins 2 (`admin-token`) and 3 (`admin3-token`), and user 4
/// (`user-token`, `phone-token`), each with [`PASSWORD`]; user 5 (`other-token`); and user 6,
/// being deleted.
async fn accounts() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app("");
    let conn = conn(&temp);
    let password = hash_new(PASSWORD.to_owned(), Vec::new()).await.unwrap();
    conn.execute("UPDATE users SET password = ?1 WHERE id = 1", [&password]).unwrap();
    for (id, role) in [(2, "admin"), (3, "admin"), (4, "user")] {
        User { password: password.clone(), ..User::new(id, role) }.insert(&conn).unwrap();
    }
    add_user(&conn, 5, "user");
    add_user(&conn, 6, "user");
    add_device(&conn, 2, 2, "admin-token");
    add_device(&conn, 3, 3, "admin3-token");
    add_device(&conn, 4, 4, "user-token");
    add_device(&conn, 4, 7, "phone-token");
    add_device(&conn, 5, 5, "other-token");
    mark_deleting(&conn, 6).unwrap();
    (temp, db, app)
}

/// `adminIssuePasswordReset` for account `user` as `token`, confirmed with `password`.
async fn issue(app: &Router, token: &str, user: i64, password: &str) -> (StatusCode, Value) {
    let uri = format!("/api/v1/admin/users/{user}/password-reset");
    call(app, token, "POST", &uri, Some(json!({ "password": password }))).await
}

/// A code the owner issues for account `user`.
async fn issued(app: &Router, user: i64) -> String {
    let (status, body) = issue(app, TOKEN, user, PASSWORD).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["code"].as_str().unwrap().to_owned()
}

/// Sends `body` to `uri` with no credentials, and answers the status, the headers, and the
/// JSON body: `null` when there is none.
async fn public(app: &Router, uri: &str, body: Value) -> (StatusCode, HeaderMap, Value) {
    let request = Request::post(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, headers, body) = respond(app.clone(), request).await;
    let body = if body.is_empty() { Value::Null } else { serde_json::from_slice(&body).unwrap() };
    (status, headers, body)
}

/// `checkPasswordReset` for `code`.
async fn check(app: &Router, code: &str) -> (StatusCode, Value) {
    let (status, _, body) = public(app, CHECK, json!({ "code": code })).await;
    (status, body)
}

/// `redeemPasswordReset` for `code`, setting `password`.
async fn redeem(app: &Router, code: &str, password: &str) -> (StatusCode, Value) {
    let (status, _, body) =
        public(app, REDEEM, json!({ "code": code, "password": password })).await;
    (status, body)
}

/// The status of logging in as `username` with `password`.
async fn login(app: &Router, username: &str, password: &str) -> StatusCode {
    let body = json!({
        "username": username,
        "password": password,
        "device": { "name": "Firefox on Linux", "type": "desktop" }
    });
    public(app, "/api/v1/auth/login", body).await.0
}

/// Account `user`'s reset columns: the hash and when it expires.
fn reset(conn: &Connection, user: i64) -> (Option<Vec<u8>>, Option<i64>) {
    conn.query_row("SELECT reset_hash, reset_expires_at FROM users WHERE id = ?1", [user], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })
    .unwrap()
}

/// Whether any account has a reset link.
fn any_reset(conn: &Connection) -> bool {
    conn.query_row("SELECT count(*) FROM users WHERE reset_hash IS NOT NULL", [], |row| {
        row.get::<_, i64>(0)
    })
    .unwrap()
        > 0
}

/// Account `user`'s stored password hash.
fn stored(conn: &Connection, user: i64) -> String {
    conn.query_row("SELECT password FROM users WHERE id = ?1", [user], |row| row.get(0)).unwrap()
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

/// The audit rows of redemptions, as (actor, target, detail).
fn used(conn: &Connection) -> Vec<(Option<i64>, i64, Value)> {
    conn.prepare(
        "SELECT actor_id, target_id, detail FROM admin_actions \
         WHERE action = 'password_reset_used' ORDER BY id",
    )
    .unwrap()
    .query_map([], |row| {
        let detail: String = row.get(2)?;
        Ok((row.get(0)?, row.get(1)?, serde_json::from_str(&detail).unwrap()))
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// How many sign-in attempts are recorded, password confirmations among them.
fn attempts(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM login_attempts", [], |row| row.get(0)).unwrap()
}

/// Whether the admins' list marks account `user` as having a reset pending.
async fn pending(app: &Router, user: &str) -> bool {
    let (status, page) = call(app, TOKEN, "GET", "/api/v1/admin/users", None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    let account = items.iter().find(|item| item["id"] == user).unwrap();
    account["resetPending"].as_bool().unwrap()
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
async fn issuing_answers_a_code_and_changes_nothing_else() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let password = stored(&conn, 4);

    let before = now_ms();
    let (status, body) = issue(&app, "admin-token", 4, PASSWORD).await;
    let after = now_ms();
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let code = body["code"].as_str().unwrap().to_owned();
    let (hash, expires_at) = reset(&conn, 4);
    let expires_at = expires_at.unwrap();
    assert_eq!(body, json!({ "code": code, "expiresAt": rfc3339(expires_at) }));
    assert!((before + LIFETIME_MS..=after + LIFETIME_MS).contains(&expires_at));
    assert_eq!(hash.unwrap(), token_hash(&code));

    // The code itself is nowhere in the database's files, where a name it holds is.
    let files = ["jewelcase.db", "jewelcase.db-wal"]
        .map(|file| std::fs::read(temp.path().join(file)).unwrap_or_default())
        .concat();
    let holds = |text: &str| files.windows(text.len()).any(|bytes| bytes == text.as_bytes());
    assert!(holds("user4"));
    assert!(!holds(&code));

    assert!(pending(&app, "4").await);
    assert_eq!(check(&app, &code).await, (StatusCode::OK, json!({ "username": "user4" })));
    // A second link replaces the first.
    let (status, second) = issue(&app, TOKEN, 4, PASSWORD).await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    assert_eq!(check(&app, &code).await.0, StatusCode::NOT_FOUND);
    assert_eq!(check(&app, second["code"].as_str().unwrap()).await.0, StatusCode::OK);

    assert_eq!(
        audit(&conn),
        [
            ("password_reset_issued".to_owned(), 2, 4, json!({})),
            ("password_reset_issued".to_owned(), 1, 4, json!({}))
        ]
    );
    assert_eq!(stored(&conn, 4), password);
    assert!(revoked(&conn).iter().all(|(_, reason)| reason.is_none()));
    assert_eq!(call(&app, "phone-token", "GET", "/api/v1/me", None).await.0, StatusCode::OK);
    assert_eq!(login(&app, "user4", PASSWORD).await, StatusCode::OK);
}

#[tokio::test]
async fn issuing_ignores_an_idempotency_key() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let mut codes = Vec::new();
    for _ in 0..2 {
        let request = Request::post("/api/v1/admin/users/4/password-reset")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header("idempotency-key", "issue-once")
            .body(Body::from(json!({ "password": PASSWORD }).to_string()))
            .unwrap();
        let (status, _, body) = respond(app.clone(), request).await;
        assert_eq!(status, StatusCode::CREATED);
        let body: Value = serde_json::from_slice(&body).unwrap();
        codes.push(body["code"].as_str().unwrap().to_owned());
    }
    assert_ne!(codes[0], codes[1]);
    let keys: i64 =
        conn.query_row("SELECT count(*) FROM idempotency_keys", [], |row| row.get(0)).unwrap();
    assert_eq!(keys, 0);
    assert_eq!(reset(&conn, 4).0.unwrap(), token_hash(&codes[1]));
}

#[tokio::test]
async fn issuing_needs_the_callers_own_password() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let (status, problem) = issue(&app, "admin-token", 4, "wrong horse").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    assert_eq!(problem["code"], "wrong_password");
    assert_eq!(problem["detail"], "Your current password is not correct.");
    assert_eq!(reset(&conn, 4), (None, None));
    assert!(audit(&conn).is_empty());
}

#[tokio::test]
async fn issuing_obeys_the_rank_rule() {
    let (temp, _db, app) = accounts().await;
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
        let (status, problem) = issue(&app, token, user, PASSWORD).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{token} on {user}: {problem}");
        assert_eq!(problem["code"], "forbidden", "{token} on {user}");
        assert_eq!(problem["detail"], detail, "{token} on {user}");
    }
    for user in [99, 6] {
        let (status, problem) = issue(&app, TOKEN, user, PASSWORD).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{user}: {problem}");
        assert_eq!(
            problem,
            json!({ "type": "about:blank", "title": "Not Found", "status": 404,
                                    "code": "not_found" })
        );
    }
    let (status, problem) = issue(&app, "other-token", 4, PASSWORD).await;
    assert_eq!((status, &problem["code"]), (StatusCode::FORBIDDEN, &json!("forbidden")));
    assert!(!any_reset(&conn));
    assert!(audit(&conn).is_empty());

    // The owner manages admins, and a suspended account may be given a link.
    conn.execute("UPDATE users SET status = 'suspended' WHERE id = 3", []).unwrap();
    assert_eq!(issue(&app, TOKEN, 3, PASSWORD).await.0, StatusCode::CREATED);
}

/// What happens to admin 2 between their password being checked and the issuing write.
enum Meanwhile {
    Suspended,
    Demoted,
    PasswordChanged,
}

/// Issues a link for user 4 as admin 2 with their password check held while `meanwhile`
/// happens, and asserts that the request is refused with `status`, `code` and `detail`, and
/// issues nothing.
async fn issue_stops_at(meanwhile: Meanwhile, status: StatusCode, code: &str, detail: Value) {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let elsewhere = hash_new(NEW.to_owned(), Vec::new()).await.unwrap();
    let before = attempts(&conn);

    let held = hold_checks().await;
    let racing = tokio::spawn({
        let app = app.clone();
        async move { issue(&app, "admin-token", 4, PASSWORD).await }
    });
    eventually("the attempt is recorded", async || (attempts(&conn) > before).then_some(())).await;
    match meanwhile {
        Meanwhile::Suspended => {
            let (uri, body) = ("/api/v1/admin/users/2/status", json!({ "status": "suspended" }));
            assert_eq!(call(&app, TOKEN, "PUT", uri, Some(body)).await.0, StatusCode::OK);
        }
        Meanwhile::Demoted => {
            conn.execute("UPDATE users SET role = 'user' WHERE id = 2", []).unwrap();
        }
        Meanwhile::PasswordChanged => {
            conn.execute("UPDATE users SET password = ?1 WHERE id = 2", [&elsewhere]).unwrap();
        }
    }
    drop(held);

    let (answered, problem) = racing.await.unwrap();
    assert_eq!((answered, &problem["code"]), (status, &json!(code)), "{problem}");
    assert_eq!(problem.get("detail").cloned().unwrap_or(Value::Null), detail);
    assert_eq!(reset(&conn, 4), (None, None));
    assert!(audit(&conn).iter().all(|(action, ..)| action != "password_reset_issued"));
}

/// Answered as the gate would now answer the suspended admin's token.
#[tokio::test]
async fn an_admin_suspended_while_issuing_issues_nothing() {
    let unauthenticated = StatusCode::UNAUTHORIZED;
    issue_stops_at(Meanwhile::Suspended, unauthenticated, "unauthenticated", Value::Null).await;
}

#[tokio::test]
async fn an_admin_demoted_while_issuing_issues_nothing() {
    let detail = json!("Only an active admin or the owner can manage accounts.");
    issue_stops_at(Meanwhile::Demoted, StatusCode::FORBIDDEN, "forbidden", detail).await;
}

#[tokio::test]
async fn an_admin_whose_password_changed_while_issuing_issues_nothing() {
    let detail = json!("Your current password is not correct.");
    issue_stops_at(Meanwhile::PasswordChanged, StatusCode::FORBIDDEN, "wrong_password", detail)
        .await;
}

#[tokio::test]
async fn redeeming_sets_the_password_and_signs_out_every_device() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let address = serve(app.clone()).await;
    let mut laptop = connect_with(address, "user-token").await;
    let mut phone = connect_with(address, "phone-token").await;
    let mut other = connect_with(address, "other-token").await;
    let code = issued(&app, 4).await;

    let (status, headers, body) =
        public(&app, REDEEM, json!({ "code": code, "password": NEW })).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    assert!(headers.get(header::SET_COOKIE).is_none(), "{headers:?}");

    for token in ["user-token", "phone-token"] {
        let (status, problem) = call(&app, token, "GET", "/api/v1/me", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{token}");
        assert_eq!(
            (&problem["code"], &problem["reason"]),
            (&json!("session_revoked"), &json!("credentials_changed")),
            "{token}"
        );
    }
    signed_out(&mut laptop).await;
    signed_out(&mut phone).await;
    stays_signed_in(&mut other).await;
    assert_eq!(call(&app, "other-token", "GET", "/api/v1/me", None).await.0, StatusCode::OK);

    assert!(!pending(&app, "4").await);
    assert_eq!(used(&conn), [(None, 4, json!({}))]);
    assert_eq!(check(&app, &code).await.0, StatusCode::NOT_FOUND);
    assert_eq!(redeem(&app, &code, OTHER_NEW).await.0, StatusCode::NOT_FOUND);
    assert_eq!(login(&app, "user4", PASSWORD).await, StatusCode::UNAUTHORIZED);
    assert_eq!(login(&app, "user4", NEW).await, StatusCode::OK);
}

#[tokio::test]
async fn a_redemption_whose_request_is_dropped_mid_write_still_closes_the_sockets() {
    let (_temp, db, app) = accounts().await;
    let address = serve(app.clone()).await;
    let mut phone = connect_with(address, "phone-token").await;
    let code = issued(&app, 4).await;

    let (held, release) = hold_next_password_write(&db).await;
    let request = tokio::spawn({
        let app = app.clone();
        async move { redeem(&app, &code, NEW).await }
    });
    held.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    drop(release);
    signed_out(&mut phone).await;
}

/// Redeems user 4's link with a password of its own, holds it after its live check, where it
/// asks to hash that password, runs `meanwhile`'s statement, and asserts that the redemption
/// is `404` and changes nothing: only the write's guard can stop it there.
async fn redeem_stops_at(meanwhile: &str) {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let code = issued(&app, 4).await;
    let password = unique_password();

    let held = hold_checks().await;
    let racing = tokio::spawn({
        let (app, password) = (app.clone(), password.clone());
        async move { redeem(&app, &code, &password).await }
    });
    past_its_live_check(&password).await;
    conn.execute_batch(meanwhile).unwrap();
    drop(held);

    let (status, problem) = racing.await.unwrap();
    assert_eq!((status, &problem["code"]), (StatusCode::NOT_FOUND, &json!("not_found")));
    assert!(!verify(password, Some(stored(&conn, 4))).await.unwrap(), "no password set");
    assert!(revoked(&conn).iter().all(|(_, reason)| reason.is_none()), "no device signed out");
    assert!(used(&conn).is_empty());
}

/// A strong password no other test hashes, so [`asked_to_hash`] speaks of this test's request.
fn unique_password() -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!("{NEW} {}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Waits until the redemption setting `password` has passed its live check and asked to hash
/// it, which it cannot finish while the test holds the hashing permits.
async fn past_its_live_check(password: &str) {
    eventually("the redemption asks to hash", async || asked_to_hash(password).then_some(())).await;
}

/// The link is left, so only the account's status stops the write.
#[tokio::test]
async fn a_redemption_checked_before_a_suspension_changes_nothing() {
    redeem_stops_at("UPDATE users SET status = 'suspended' WHERE id = 4").await;
}

#[tokio::test]
async fn a_redemption_checked_before_a_new_link_changes_nothing() {
    let statement = format!(
        "UPDATE users SET reset_hash = X'{}', reset_expires_at = {} WHERE id = 4",
        token_hash("newer").iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
        now_ms() + LIFETIME_MS,
    );
    redeem_stops_at(&statement).await;
}

#[tokio::test]
async fn a_redemption_checked_before_its_link_expired_changes_nothing() {
    redeem_stops_at(&format!("UPDATE users SET reset_expires_at = {} WHERE id = 4", now_ms() - 1))
        .await;
}

/// As `changeMyPassword` writes it.
#[tokio::test]
async fn a_redemption_checked_before_a_password_change_changes_nothing() {
    redeem_stops_at(
        "UPDATE users SET password = 'changed', reset_hash = NULL, reset_expires_at = NULL \
         WHERE id = 4",
    )
    .await;
}

/// Both pass the live check before either hashes, so the second is stopped by its write alone.
#[tokio::test]
async fn of_two_redemptions_at_once_one_applies() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let code = issued(&app, 4).await;
    let passwords = [unique_password(), unique_password()];

    let held = hold_checks().await;
    let redemptions = passwords.clone().map(|password| {
        let (app, code) = (app.clone(), code.clone());
        tokio::spawn(async move { redeem(&app, &code, &password).await })
    });
    for password in &passwords {
        past_its_live_check(password).await;
    }
    drop(held);

    let mut answers = Vec::new();
    for redemption in redemptions {
        answers.push(redemption.await.unwrap().0);
    }
    answers.sort();
    assert_eq!(answers, [StatusCode::NO_CONTENT, StatusCode::NOT_FOUND]);
    assert_eq!(used(&conn).len(), 1);
    let [first, second] = passwords;
    let signs_in = [login(&app, "user4", &first).await, login(&app, "user4", &second).await];
    assert_eq!(signs_in.iter().filter(|status| **status == StatusCode::OK).count(), 1);
}

/// Every link that cannot be used answers the same `404` from both operations, and redeeming
/// one hashes nothing: a weak password would otherwise be `422`.
#[tokio::test]
async fn a_dead_link_is_refused_alike_before_any_hashing() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let used_code = issued(&app, 4).await;
    assert_eq!(redeem(&app, &used_code, NEW).await.0, StatusCode::NO_CONTENT);
    let replaced = issued(&app, 4).await;
    issued(&app, 4).await;
    let expired = issued(&app, 3).await;
    conn.execute("UPDATE users SET reset_expires_at = ?1 WHERE id = 3", [now_ms() - 1]).unwrap();
    let suspended = issued(&app, 5).await;
    conn.execute("UPDATE users SET status = 'suspended' WHERE id = 5", []).unwrap();

    let (_, unknown) = check(&app, "nope").await;
    assert_eq!(
        unknown,
        json!({ "type": "about:blank", "title": "Not Found", "status": 404, "code": "not_found" })
    );
    for code in ["nope", &*used_code, &replaced, &expired, &suspended] {
        assert_eq!(check(&app, code).await, (StatusCode::NOT_FOUND, unknown.clone()), "{code}");
        for password in [OTHER_NEW, "password"] {
            let answer = redeem(&app, code, password).await;
            assert_eq!(answer, (StatusCode::NOT_FOUND, unknown.clone()), "{code}");
        }
    }
    assert_eq!(used(&conn).len(), 1);
}

#[tokio::test]
async fn a_weak_new_password_leaves_the_link_live() {
    let (temp, _db, app) = accounts().await;
    let conn = conn(&temp);
    let code = issued(&app, 4).await;
    let (status, problem) = redeem(&app, &code, "password").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "weak_password");
    assert_eq!(
        problem["detail"],
        "This is a top-10 common password. Choose a longer or less predictable password."
    );
    // Strong alone, but the account's own display name.
    conn.execute("UPDATE users SET display_name = ?1 WHERE id = 4", [NEW]).unwrap();
    let (status, problem) = redeem(&app, &code, NEW).await;
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::UNPROCESSABLE_ENTITY, &json!("weak_password"))
    );
    assert_eq!(check(&app, &code).await.0, StatusCode::OK);
    assert!(used(&conn).is_empty());
    assert_eq!(login(&app, "user4", PASSWORD).await, StatusCode::OK);
}

#[tokio::test]
async fn suspending_an_account_kills_its_link() {
    let (_temp, _db, app) = accounts().await;
    let code = issued(&app, 4).await;
    for status in ["suspended", "active"] {
        let body = json!({ "status": status });
        let uri = "/api/v1/admin/users/4/status";
        assert_eq!(call(&app, TOKEN, "PUT", uri, Some(body)).await.0, StatusCode::OK);
    }
    assert_eq!(check(&app, &code).await.0, StatusCode::NOT_FOUND);
    assert_eq!(redeem(&app, &code, NEW).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_check_names_only_the_links_account() {
    let (_temp, _db, app) = accounts().await;
    let code = issued(&app, 4).await;
    issued(&app, 5).await;
    assert_eq!(check(&app, &code).await, (StatusCode::OK, json!({ "username": "user4" })));
}

#[tokio::test]
async fn both_wait_for_setup() {
    let (_temp, _db, app) = app_before_setup("");
    for (uri, body) in
        [(CHECK, json!({ "code": "c" })), (REDEEM, json!({ "code": "c", "password": NEW }))]
    {
        let (status, _, problem) = public(&app, uri, body).await;
        assert_eq!(
            (status, &problem["code"]),
            (StatusCode::SERVICE_UNAVAILABLE, &json!("setup_required")),
            "{uri}"
        );
    }
}
