use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use serde_json::{Value, json};

use super::super::testing::{app_before_setup, respond, send_json};
use crate::db::Database;

const PASSWORD: &str = "correct horse battery staple";

/// A server whose owner is `sam` with [`PASSWORD`], set up the way a person would.
async fn server() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app_before_setup("");
    let request = json!({
        "username": "sam",
        "password": PASSWORD,
        "device": { "name": "Laptop", "type": "desktop" }
    });
    let (status, _, _) = send_json(app.clone(), "POST", "/api/v1/setup", &request).await;
    assert_eq!(status, StatusCode::CREATED);
    (temp, db, app)
}

/// Logs in from `peer`, and returns the status, headers, and JSON body.
async fn login(
    app: &Router,
    peer: &str,
    username: &str,
    password: &str,
) -> (StatusCode, HeaderMap, Value) {
    let body = json!({
        "username": username,
        "password": password,
        "device": { "name": "Firefox on Linux", "type": "desktop", "platform": "web" }
    });
    let request = Request::post("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .extension(ConnectInfo(format!("{peer}:4000").parse::<SocketAddr>().unwrap()))
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, headers, body) = respond(app.clone(), request).await;
    (status, headers, serde_json::from_slice(&body).unwrap())
}

/// Moves every recorded attempt `ms` into the past, as if that much time had gone by.
async fn wait(db: &Database, ms: i64) {
    db.write(move |tx| tx.execute("UPDATE login_attempts SET at = at - ?1", [ms])).await.unwrap();
}

#[tokio::test]
async fn logs_in_on_a_new_device() {
    let (_temp, db, app) = server().await;
    // Usernames are unique regardless of case.
    let (status, headers, session) = login(&app, "203.0.113.5", "SAM", PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{session}");
    assert_eq!(session["user"]["username"], "sam");
    assert_eq!(session["device"]["name"], "Firefox on Linux");
    let token = session["token"].as_str().unwrap();
    let cookie = headers[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with(&format!("jewelcase_session={token};")), "{cookie}");

    let me = Request::get("/api/v1/me")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(respond(app, me).await.0, StatusCode::OK);
    let devices: i64 = db
        .read(|conn| conn.query_row("SELECT count(*) FROM devices", [], |row| row.get(0)))
        .await
        .unwrap();
    assert_eq!(devices, 2, "setup's and this one");
}

#[tokio::test]
async fn every_refusal_is_the_same() {
    let (_temp, db, app) = server().await;
    db.write(|tx| {
        tx.execute(
            "INSERT INTO users (id, username, display_name, role, status, password, created_at, \
             updated_at) SELECT 2, 'away', 'Away', 'user', 'suspended', password, 0, 0 \
             FROM users WHERE username = 'sam'",
            [],
        )
    })
    .await
    .unwrap();
    let refusals = [
        login(&app, "203.0.113.5", "sam", "wrong horse battery staple").await,
        login(&app, "203.0.113.5", "nobody", PASSWORD).await,
        // The right password, but the account is suspended.
        login(&app, "203.0.113.5", "away", PASSWORD).await,
    ];
    for (status, headers, problem) in refusals {
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            problem,
            json!({ "type": "about:blank", "title": "Unauthorized", "status": 401,
                    "code": "invalid_credentials" })
        );
        assert!(headers.get(header::SET_COOKIE).is_none());
    }
}

#[tokio::test]
async fn attempts_are_recorded() {
    let (_temp, db, app) = server().await;
    login(&app, "203.0.113.5", "sam", "wrong").await;
    login(&app, "198.51.100.7", "Ghost", PASSWORD).await;
    login(&app, "203.0.113.5", "sam", PASSWORD).await;
    let rows: Vec<(String, bool, String, bool)> = db
        .read(|conn| {
            conn.prepare(
                "SELECT username_attempted, user_id IS NOT NULL, origin, succeeded \
                 FROM login_attempts ORDER BY id",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .collect()
        })
        .await
        .unwrap();
    let row = |name: &str, known, origin: &str, succeeded| {
        (name.to_owned(), known, origin.to_owned(), succeeded)
    };
    assert_eq!(
        rows,
        [
            row("sam", true, "203.0.113.5", false),
            row("Ghost", false, "198.51.100.7", false),
            row("sam", true, "203.0.113.5", true),
        ]
    );
}

#[tokio::test]
async fn repeated_failures_wait_longer_each_time_then_clear() {
    let (_temp, db, app) = server().await;
    for _ in 0..5 {
        assert_eq!(login(&app, "203.0.113.5", "sam", "wrong").await.0, StatusCode::UNAUTHORIZED);
    }
    // Refused without checking, so even the right password waits, from anywhere.
    let (status, headers, problem) = login(&app, "198.51.100.7", "sam", PASSWORD).await;
    assert_eq!((status, &problem["code"]), (StatusCode::TOO_MANY_REQUESTS, &json!("rate_limited")));
    assert_eq!(headers[header::RETRY_AFTER], "1");

    wait(&db, 1000).await;
    assert_eq!(login(&app, "203.0.113.5", "sam", "wrong").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(login(&app, "203.0.113.5", "sam", PASSWORD).await.1[header::RETRY_AFTER], "2");

    // An hour on, the failures no longer count.
    wait(&db, 60 * 60 * 1000).await;
    assert_eq!(login(&app, "203.0.113.5", "sam", PASSWORD).await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_success_clears_the_accounts_failures() {
    let (_temp, _db, app) = server().await;
    for _ in 0..4 {
        login(&app, "203.0.113.5", "sam", "wrong").await;
    }
    assert_eq!(login(&app, "203.0.113.5", "sam", PASSWORD).await.0, StatusCode::OK);
    for _ in 0..4 {
        login(&app, "203.0.113.5", "sam", "wrong").await;
    }
    assert_eq!(login(&app, "203.0.113.5", "sam", PASSWORD).await.0, StatusCode::OK);
}

#[tokio::test]
async fn unknown_usernames_are_limited_like_real_ones() {
    let (_temp, _db, app) = server().await;
    for _ in 0..5 {
        login(&app, "203.0.113.5", "ghost", "wrong").await;
    }
    assert_eq!(login(&app, "203.0.113.5", "GHOST", "wrong").await.0, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_sweep_from_one_origin_is_limited() {
    let (_temp, _db, app) = server().await;
    for i in 0..20 {
        let (status, _, _) = login(&app, "203.0.113.5", &format!("user{i}"), "wrong").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(login(&app, "203.0.113.5", "sam", PASSWORD).await.0, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(login(&app, "198.51.100.7", "sam", PASSWORD).await.0, StatusCode::OK);
}

#[tokio::test]
async fn attempts_made_at_once_count_against_each_other() {
    let (_temp, _db, app) = server().await;
    let mut attempts = tokio::task::JoinSet::new();
    for _ in 0..12 {
        let app = app.clone();
        attempts.spawn(async move { login(&app, "203.0.113.5", "sam", "wrong").await.0 });
    }
    let statuses = attempts.join_all().await;
    let checked = statuses.iter().filter(|status| **status == StatusCode::UNAUTHORIZED).count();
    assert_eq!(checked, 5, "{statuses:?}");
}

#[tokio::test]
async fn login_waits_for_setup() {
    let (_temp, _db, app) = app_before_setup("");
    let (status, _, problem) = login(&app, "203.0.113.5", "sam", PASSWORD).await;
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::SERVICE_UNAVAILABLE, &json!("setup_required"))
    );
}

#[tokio::test]
async fn a_malformed_username_is_a_validation_problem() {
    let (_temp, _db, app) = server().await;
    let (status, _, problem) = login(&app, "203.0.113.5", &"a".repeat(1000), PASSWORD).await;
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::UNPROCESSABLE_ENTITY, &json!("validation_failed"))
    );
}
