//! Logging in with the `deviceId` of a device a password change signed out.

use axum::Router;
use axum::http::StatusCode;
use rusqlite::Connection;
use serde_json::{Value, json};

use super::super::super::password::{hash_new, hold_checks};
use super::super::super::testing::socket::{connect_with, next_of, serve};
use super::super::super::testing::{
    add_device, add_signed_out_device, call, conn, eventually, send_json,
};
use super::{PASSWORD, server};
use crate::db::testing::User;

/// A server whose owner `sam` (the first account) has device 20 signed out by a password change,
/// first seen at the epoch and named "Old phone"; device 21 signed in, 22 logged out, and 23 signed
/// out by a password change; and account 2, with its own device 30 signed out by one.
async fn accounts() -> (tempfile::TempDir, Router, i64) {
    let (temp, _db, app) = server().await;
    let conn = conn(&temp);
    let sam: i64 =
        conn.query_row("SELECT id FROM users WHERE username = 'sam'", [], |r| r.get(0)).unwrap();
    let password = hash_new(PASSWORD.to_owned(), Vec::new()).await.unwrap();
    User { password, ..User::new(2, "user") }.insert(&conn).unwrap();
    add_signed_out_device(&conn, sam, 20, "old-20", "credentials_changed");
    conn.execute("UPDATE devices SET name = 'Old phone', type = 'phone' WHERE id = 20", [])
        .unwrap();
    add_device(&conn, sam, 21, "live-21");
    add_signed_out_device(&conn, sam, 22, "old-22", "logged_out");
    add_signed_out_device(&conn, sam, 23, "old-23", "credentials_changed");
    add_signed_out_device(&conn, 2, 30, "old-30", "credentials_changed");
    (temp, app, sam)
}

/// Logs `username` in with `password`, naming `device_id`.
async fn login_as(app: &Router, password: &str, device_id: Value) -> (StatusCode, Value) {
    let body = json!({
        "username": "sam",
        "password": password,
        "device": { "name": "Firefox on Linux", "type": "desktop", "platform": "web" },
        "deviceId": device_id
    });
    let (status, _, body) = send_json(app.clone(), "POST", "/api/v1/auth/login", &body).await;
    (status, body)
}

/// Every column of device `id`, hash included.
fn row(conn: &Connection, id: i64) -> Vec<rusqlite::types::Value> {
    conn.query_row("SELECT * FROM devices WHERE id = ?1", [id], |r| {
        (0..r.as_ref().column_count()).map(|i| r.get(i)).collect()
    })
    .unwrap()
}

#[tokio::test]
async fn a_signed_out_device_returns_as_itself() {
    let (temp, app, _) = accounts().await;
    let (status, session) = login_as(&app, PASSWORD, json!("20")).await;
    assert_eq!(status, StatusCode::OK, "{session}");

    let device = &session["device"];
    assert_eq!(device["id"], "20");
    assert_eq!(device["name"], "Old phone");
    assert_eq!(device["firstSeenAt"], "1970-01-01T00:00:00.000Z");
    assert_ne!(device["lastSeenAt"], "1970-01-01T00:00:00.000Z");
    assert_eq!((&device["type"], &device["platform"]), (&json!("desktop"), &json!("web")));

    let token = session["token"].as_str().unwrap();
    assert_eq!(call(&app, token, "GET", "/api/v1/me", None).await.0, StatusCode::OK);
    // The row's token hash is replaced, so the old token is no one's.
    let (status, old) = call(&app, "old-20", "GET", "/api/v1/me", None).await;
    assert_eq!((status, &old["code"]), (StatusCode::UNAUTHORIZED, &json!("unauthenticated")));

    let count: i64 = conn(&temp)
        .query_row("SELECT count(*) FROM devices WHERE user_id != 2", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 5, "setup's, 20 to 23 and no new one");
}

#[tokio::test]
async fn any_other_id_registers_a_new_device_and_leaves_the_named_one_alone() {
    let ids = [
        json!("999"),
        json!("twenty"),
        json!(""),
        json!("99999999999999999999"),
        json!("21"),
        json!("22"),
        json!("30"),
    ];
    for id in ids {
        let (temp, app, _) = accounts().await;
        let conn = conn(&temp);
        let before = id.as_str().and_then(|s| s.parse().ok()).map(|n| row_or_none(&conn, n));
        let (status, session) = login_as(&app, PASSWORD, id.clone()).await;
        assert_eq!(status, StatusCode::OK, "{id}: {session}");
        let new = session["device"]["id"].as_str().unwrap();
        assert!(!["20", "21", "22", "30"].contains(&new), "{id}: {session}");
        assert_eq!(session["device"]["name"], "Firefox on Linux", "{id}");
        if let (Some(before), Some(n)) = (before, id.as_str().and_then(|s| s.parse::<i64>().ok())) {
            assert_eq!(row_or_none(&conn, n), before, "{id}");
        }
    }
}

/// Device `id`'s row, or nothing if there is none.
fn row_or_none(conn: &Connection, id: i64) -> Vec<rusqlite::types::Value> {
    if conn
        .query_row("SELECT count(*) FROM devices WHERE id = ?1", [id], |r| r.get::<_, i64>(0))
        .unwrap()
        == 0
    {
        return Vec::new();
    }
    row(conn, id)
}

#[tokio::test]
async fn a_wrong_password_reclaims_nothing() {
    let (temp, app, _) = accounts().await;
    let conn = conn(&temp);
    let before = row(&conn, 20);
    let (status, problem) = login_as(&app, "not the password", json!("20")).await;
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::UNAUTHORIZED, &json!("invalid_credentials"))
    );
    assert_eq!(row(&conn, 20), before);
}

/// A login that checked the password a change then replaced must not sign in after it, or the
/// change's sign-out would be undone: neither as the reclaimed device nor as a new one.
#[tokio::test]
async fn a_sign_in_whose_password_check_straddles_a_password_change_is_refused() {
    let (temp, app, sam) = accounts().await;
    let conn = conn(&temp);
    let attempts = || -> i64 {
        conn.query_row("SELECT count(*) FROM login_attempts", [], |row| row.get(0)).unwrap()
    };
    let (before, devices) = (attempts(), row(&conn, 20));
    let count =
        || -> i64 { conn.query_row("SELECT count(*) FROM devices", [], |row| row.get(0)).unwrap() };
    let devices_before = count();
    let changed = hash_new("an entirely different passphrase".to_owned(), Vec::new()).await;

    let held = hold_checks().await;
    let racing = tokio::spawn({
        let app = app.clone();
        async move { login_as(&app, PASSWORD, json!("20")).await }
    });
    eventually("the attempt is recorded", async || (attempts() > before).then_some(())).await;
    let changed = rusqlite::params![changed.unwrap(), sam];
    conn.execute("UPDATE users SET password = ?1 WHERE id = ?2", changed).unwrap();
    drop(held);

    let (status, problem) = racing.await.unwrap();
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::UNAUTHORIZED, &json!("invalid_credentials"))
    );
    assert_eq!(row(&conn, 20), devices);
    assert_eq!(count(), devices_before);
}

#[tokio::test]
async fn a_device_reclaimed_twice_at_once_is_signed_in_once() {
    let (temp, app, _) = accounts().await;
    let (first, second) =
        tokio::join!(login_as(&app, PASSWORD, json!("20")), login_as(&app, PASSWORD, json!("20")));
    let mut ids = [first.1["device"]["id"].clone(), second.1["device"]["id"].clone()];
    ids.sort_by_key(|id| id.as_str().map(str::to_owned));
    assert_eq!(ids.iter().filter(|id| **id == json!("20")).count(), 1, "{ids:?}");
    let count: i64 = conn(&temp)
        .query_row("SELECT count(*) FROM devices WHERE user_id != 2", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 6, "setup's, 20 to 23 and one new");
}

#[tokio::test]
async fn the_accounts_connected_devices_are_sent_the_new_list() {
    let (temp, app, _) = accounts().await;
    let address = serve(app.clone()).await;
    // Only the signed-in device holds a socket, so a list with 20 in it comes from the login.
    let mut socket = connect_with(address, "live-21").await;
    let first = next_of(&mut socket, "devices").await;
    let ids = |list: &Value| -> Vec<Value> {
        list["devices"].as_array().unwrap().iter().map(|d| d["id"].clone()).collect()
    };
    assert!(!ids(&first).contains(&json!("20")), "{first}");

    assert_eq!(login_as(&app, PASSWORD, json!("20")).await.0, StatusCode::OK);
    loop {
        if ids(&next_of(&mut socket, "devices").await).contains(&json!("20")) {
            break;
        }
    }
    drop(temp);
}
