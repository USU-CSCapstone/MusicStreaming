//! A signed-out device's token, which the database keeps: told why on every route, by bearer
//! token or cookie, and never let through. A suspended account's looks exactly as an unknown
//! token does (`requirements/users.md` §10).

use axum::Router;
use axum::http::{StatusCode, header};
use serde_json::{Value, json};

use super::super::testing::{
    add_signed_out_device, app, app_before_setup, app_with_tracks, conn, send_json,
};
use super::gates::{ROUTES, send};
use crate::db::testing::User;

const PASSWORD: &str = "correct horse battery staple";

/// [`app_with_tracks`] with user 2, who reaches no library, and suspended user 3. User 2 is on
/// device 2, with token `revoked`, signed out when their credentials changed, and on device 4,
/// with token `logged-out`, signed out remotely. User 3 is on device 3, with token
/// `suspended`, signed out when their credentials changed.
async fn server() -> (tempfile::TempDir, Router) {
    let (temp, _db, app) = app_with_tracks().await;
    let conn = conn(&temp);
    User::new(2, "user").insert(&conn).unwrap();
    User { status: "suspended", ..User::new(3, "user") }.insert(&conn).unwrap();
    add_signed_out_device(&conn, 2, 2, "revoked", "credentials_changed");
    add_signed_out_device(&conn, 2, 4, "logged-out", "logged_out");
    add_signed_out_device(&conn, 3, 3, "suspended", "credentials_changed");
    (temp, app)
}

/// `token` as a bearer token and as the session cookie, the latter without
/// `X-Jewelcase-Client`.
fn credentials(token: &str) -> [(&'static str, String); 2] {
    [("authorization", format!("Bearer {token}")), ("cookie", format!("jewelcase_session={token}"))]
}

#[tokio::test]
async fn a_signed_out_devices_token_is_told_why_on_every_route() {
    let (_temp, app) = server().await;
    for (token, reason) in [("revoked", "credentials_changed"), ("logged-out", "logged_out")] {
        // Only the code and the reason: nothing about the account or the device.
        let expected = json!({ "type": "about:blank", "title": "Unauthorized", "status": 401,
                               "code": "session_revoked", "reason": reason });
        for (name, value) in credentials(token) {
            // Past the gate, a user would be refused under `/admin`, find library 2 missing,
            // and be refused a cookie write without the client header.
            for &(method, path) in ROUTES {
                let at = format!("{method} {path} by {name} {token}");
                let (status, headers, body) = send(&app, "", method, path, &[(name, &value)]).await;
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{at}");
                assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), expected, "{at}");
                assert_eq!(headers[header::WWW_AUTHENTICATE], "Bearer", "{at}");
            }
        }
    }
}

#[tokio::test]
async fn a_suspended_accounts_signed_out_device_looks_unknown() {
    let (_temp, app) = server().await;
    for ((name, suspended), (_, unknown)) in
        credentials("suspended").into_iter().zip(credentials("unknown"))
    {
        for &(method, path) in ROUTES {
            let at = format!("{method} {path} by {name}");
            let answer = send(&app, "", method, path, &[(name, &suspended)]).await;
            assert_eq!(answer, send(&app, "", method, path, &[(name, &unknown)]).await, "{at}");
            let problem: Value = serde_json::from_slice(&answer.2).unwrap();
            assert_eq!(problem["code"], "unauthenticated", "{at}");
        }
    }
}

#[tokio::test]
async fn signing_in_again_leaves_the_old_token_signed_out() {
    let (_temp, db, app) = app_before_setup("");
    let device = json!({ "name": "Phone", "type": "phone" });
    let setup = json!({ "username": "sam", "password": PASSWORD, "device": device });
    let (_, _, session) = send_json(app.clone(), "POST", "/api/v1/setup", &setup).await;
    let old = format!("Bearer {}", session["token"].as_str().unwrap());
    db.write(|tx| tx.execute("UPDATE devices SET revoked_reason = 'credentials_changed'", []))
        .await
        .unwrap();

    let login = json!({ "username": "sam", "password": PASSWORD, "device": device });
    let (status, _, session) = send_json(app.clone(), "POST", "/api/v1/auth/login", &login).await;
    assert_eq!(status, StatusCode::OK);
    let new = format!("Bearer {}", session["token"].as_str().unwrap());
    assert_eq!(send(&app, "", "GET", "/me", &[("authorization", &new)]).await.0, StatusCode::OK);
    let (status, _, body) = send(&app, "", "GET", "/me", &[("authorization", &old)]).await;
    let problem: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!((status, &problem["code"]), (StatusCode::UNAUTHORIZED, &json!("session_revoked")));
}

#[tokio::test]
async fn a_signed_out_device_keeps_its_token_hash() {
    let (temp, _db, _app) = app("");
    let conn = conn(&temp);
    let without_hash = conn.execute(
        "INSERT INTO devices (id, user_id, name, type, revoked_reason, created_at, last_seen_at) \
         VALUES (2, 1, 'Test', 'desktop', 'credentials_changed', 0, 0)",
        [],
    );
    let error = without_hash.unwrap_err().to_string();
    assert!(error.contains("NOT NULL constraint failed: devices.token_hash"), "{error}");
}
