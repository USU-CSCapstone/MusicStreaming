//! Suspending and restoring an account.

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Json, Router};
use futures_util::SinkExt;
use rusqlite::Connection;
use rusqlite::types::Value as Column;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::super::Status;
use super::StatusChange;
use crate::api::extract::{self, Path};
use crate::api::password::{hash_new, hold_checks};
use crate::api::player;
use crate::api::scope::Caller;
use crate::api::testing::player::{start_playing, start_queue};
use crate::api::testing::socket::{Socket, connect_with, next, next_of, serve};
use crate::api::testing::{
    TOKEN, add_device, add_signed_out_device, add_user, app_with_tracks, audit, call, conn,
    eventually, hold_next, router_over, send_json, state,
};
use crate::api::{AppState, Id, session::token_hash};
use crate::db::now_ms;
use crate::db::testing::{User, access, mark_deleting};

const PASSWORD: &str = "correct horse battery staple";

/// The owner (user 1, [`TOKEN`]) over [`app_with_tracks`]; admins 2 (`admin-token`) and 3
/// (`admin3-token`); user 4 with [`PASSWORD`], reaching library 1, signed in on devices 4
/// (`user-token`) and 7 (`phone-token`) and signed out of device 8; user 5 (`other-token`); and
/// user 6, being deleted. Answers the state too, to look at playback.
async fn accounts() -> (tempfile::TempDir, AppState, Router) {
    let (temp, db, _app) = app_with_tracks().await;
    let state = state(&temp, db);
    let app = router_over(state.clone());
    let conn = conn(&temp);
    for (id, role) in [(2, "admin"), (3, "admin"), (5, "user"), (6, "user")] {
        add_user(&conn, id, role);
    }
    let password = hash_new(PASSWORD.to_owned(), Vec::new()).await.unwrap();
    User { password, ..User::new(4, "user") }.insert(&conn).unwrap();
    add_device(&conn, 2, 2, "admin-token");
    add_device(&conn, 3, 3, "admin3-token");
    add_device(&conn, 4, 4, "user-token");
    add_device(&conn, 4, 7, "phone-token");
    add_signed_out_device(&conn, 4, 8, "old-token", "credentials_changed");
    add_device(&conn, 5, 5, "other-token");
    access(&conn, 4, 1).unwrap();
    mark_deleting(&conn, 6).unwrap();
    (temp, state, app)
}

/// `adminSetUserStatus` on account `user` as `token`.
async fn set(app: &Router, token: &str, user: i64, status: &str) -> (StatusCode, Value) {
    let body = json!({ "status": status });
    call(app, token, "PUT", &format!("/api/v1/admin/users/{user}/status"), Some(body)).await
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

/// Account `user`'s status.
fn status(conn: &Connection, user: i64) -> String {
    conn.query_row("SELECT status FROM users WHERE id = ?1", [user], |row| row.get(0)).unwrap()
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

#[tokio::test]
async fn suspending_signs_out_every_device_closes_its_sockets_and_stops_playback() {
    let (temp, state, app) = accounts().await;
    start_queue(&app, "user-token").await;
    let address = serve(app.clone()).await;
    let mut playing = connect_with(address, "user-token").await;
    let mut phone = connect_with(address, "phone-token").await;
    let mut other = connect_with(address, "other-token").await;
    start_playing(&mut playing).await;

    let (answered, account) = set(&app, TOKEN, 4, "suspended").await;
    assert_eq!(answered, StatusCode::OK, "{account}");
    assert_eq!((&account["id"], &account["status"]), (&json!("4"), &json!("suspended")));
    assert_eq!(account["libraryIds"], json!(["1"]));

    let revoked = json!({ "type": "sessionRevoked", "reason": "logged_out" });
    assert_eq!(next_of(&mut playing, "sessionRevoked").await, revoked);
    assert_eq!(next_of(&mut phone, "sessionRevoked").await, revoked);
    stays_signed_in(&mut other).await;
    let conn = conn(&temp);
    assert_eq!(
        devices(&conn, 4),
        [
            (4, Some("logged_out".into())),
            (7, Some("logged_out".into())),
            (8, Some("credentials_changed".into()))
        ],
        "a device already signed out keeps its reason"
    );
    assert_eq!(devices(&conn, 5), [(5, None)]);

    let caller = Caller { user: 4, device: 4, admin: false, owner: false };
    let players = state.players.clone();
    eventually("playback stops", async || {
        let Json(current) = player::get(State(players.clone()), caller).await;
        let current = serde_json::to_value(current).unwrap();
        (current["status"] == "stopped").then_some(current)
    })
    .await;
    let Json(current) = player::get(State(players), caller).await;
    assert_eq!(serde_json::to_value(current).unwrap()["activeDeviceId"], Value::Null);
}

#[tokio::test]
async fn a_suspended_account_answers_as_an_unknown_one_and_restoring_needs_a_new_sign_in() {
    let (_temp, _state, app) = accounts().await;
    let (answered, _) = set(&app, TOKEN, 4, "suspended").await;
    assert_eq!(answered, StatusCode::OK);

    let unknown = call(&app, "no-such-token", "GET", "/api/v1/me", None).await;
    assert_eq!(unknown.0, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.1["code"], "unauthenticated");
    assert_eq!(call(&app, "user-token", "GET", "/api/v1/me", None).await, unknown);
    let wrong = login(&app, "user4", "wrong horse battery staple").await;
    assert_eq!(wrong.0, StatusCode::UNAUTHORIZED);
    assert_eq!(login(&app, "user4", PASSWORD).await, wrong);

    let (answered, account) = set(&app, TOKEN, 4, "active").await;
    assert_eq!((answered, &account["status"]), (StatusCode::OK, &json!("active")));
    assert_eq!(login(&app, "user4", PASSWORD).await.0, StatusCode::OK);
    for token in ["user-token", "phone-token"] {
        let (answered, problem) = call(&app, token, "GET", "/api/v1/me", None).await;
        assert_eq!(answered, StatusCode::UNAUTHORIZED, "{token}");
        assert_eq!(
            (&problem["code"], &problem["reason"]),
            (&json!("session_revoked"), &json!("logged_out"))
        );
    }
}

#[tokio::test]
async fn a_sign_in_whose_password_check_straddles_the_suspension_is_refused() {
    let (temp, _state, app) = accounts().await;
    let wrong = login(&app, "user4", "wrong horse battery staple").await;
    let conn = conn(&temp);
    let attempts = || -> i64 {
        conn.query_row("SELECT count(*) FROM login_attempts", [], |row| row.get(0)).unwrap()
    };
    let before = attempts();

    let held = hold_checks().await;
    let racing = tokio::spawn({
        let app = app.clone();
        async move { login(&app, "user4", PASSWORD).await }
    });
    eventually("the attempt is recorded", async || (attempts() > before).then_some(())).await;
    assert_eq!(set(&app, TOKEN, 4, "suspended").await.0, StatusCode::OK);
    drop(held);

    assert_eq!(racing.await.unwrap(), wrong);
    assert!(devices(&conn, 4).iter().all(|(_, revoked)| revoked.is_some()));
    assert_eq!(set(&app, TOKEN, 4, "active").await.0, StatusCode::OK);
    assert!(devices(&conn, 4).iter().all(|(_, revoked)| revoked.is_some()));
}

#[tokio::test]
async fn suspending_clears_the_reset_link_and_revokes_only_the_accounts_pending_invites() {
    let (temp, _state, app) = accounts().await;
    let conn = conn(&temp);
    let now = now_ms();
    conn.execute(
        "UPDATE users SET reset_hash = ?1, reset_expires_at = ?2 WHERE id = 2",
        rusqlite::params![token_hash("reset").to_vec(), now + 60_000],
    )
    .unwrap();
    // Admin 2's pending and expired invites, and admin 3's pending one.
    for (id, creator, expires_at) in
        [(10, 2, now + 60_000), (11, 2, now - 1), (12, 3, now + 60_000)]
    {
        conn.execute(
            "INSERT INTO invites (id, code_hash, created_by, created_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                id,
                token_hash(&id.to_string()).to_vec(),
                creator,
                now - 10,
                expires_at
            ],
        )
        .unwrap();
    }

    let (answered, account) = set(&app, TOKEN, 2, "suspended").await;
    assert_eq!((answered, &account["resetPending"]), (StatusCode::OK, &json!(false)));
    let reset: (Option<Vec<u8>>, Option<i64>) = conn
        .query_row("SELECT reset_hash, reset_expires_at FROM users WHERE id = 2", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(reset, (None, None));
    let revoked = |id: i64| -> Option<i64> {
        conn.query_row("SELECT revoked_at FROM invites WHERE id = ?1", [id], |row| row.get(0))
            .unwrap()
    };
    assert!(revoked(10).is_some_and(|at| at >= now), "its pending invite");
    assert_eq!(revoked(11), None, "its expired invite is left as it was");
    assert_eq!(revoked(12), None, "another admin's invite");
}

/// Every row of `table` belonging to account 4.
fn rows(conn: &Connection, table: &str) -> Vec<Vec<Column>> {
    let mut statement =
        conn.prepare(&format!("SELECT * FROM {table} WHERE user_id = 4 ORDER BY 1")).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| (0..columns).map(|i| row.get(i)).collect())
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[tokio::test]
async fn suspending_and_restoring_leave_playlists_and_plays_untouched() {
    let (temp, _state, app) = accounts().await;
    let body = Some(json!({ "title": "Mine" }));
    let made = call(&app, "user-token", "POST", "/api/v1/libraries/1/playlists", body).await;
    assert_eq!(made.0, StatusCode::CREATED, "{}", made.1);
    let play = json!({
        "playId": "5f0c6a52-3b7e-4d43-9a8e-0d1c2b3a4f5e", "libraryId": "1", "trackId": "1",
        "startedAt": "2026-10-02T12:00:00.500Z", "listenTimeMs": 1000, "end": null,
        "context": { "type": "album", "id": "101" }, "origin": "context"
    });
    let body = Some(json!({ "items": [play] }));
    let reported = call(&app, "user-token", "POST", "/api/v1/me/plays", body).await;
    assert_eq!(reported.0, StatusCode::NO_CONTENT);
    let conn = conn(&temp);
    let tables = ["playlists", "plays", "library_access"];
    let before: Vec<_> = tables.iter().map(|table| rows(&conn, table)).collect();
    assert!(before.iter().all(|rows| !rows.is_empty()), "{before:?}");

    assert_eq!(set(&app, TOKEN, 4, "suspended").await.0, StatusCode::OK);
    assert_eq!(set(&app, TOKEN, 4, "active").await.0, StatusCode::OK);
    let after: Vec<_> = tables.iter().map(|table| rows(&conn, table)).collect();
    assert_eq!(after, before);
}

#[tokio::test]
async fn only_an_account_the_caller_outranks_can_be_suspended() {
    let (temp, _state, app) = accounts().await;
    for (token, user, detail) in [
        (TOKEN, 1, "You cannot do this to your own account."),
        ("admin-token", 1, "No one can do this to the owner's account."),
        ("admin-token", 2, "You cannot do this to your own account."),
        ("admin-token", 3, "Only the owner can do this to an admin's account."),
    ] {
        let (answered, problem) = set(&app, token, user, "suspended").await;
        assert_eq!(answered, StatusCode::FORBIDDEN, "{token} on {user}: {problem}");
        assert_eq!(problem["detail"], detail, "{token} on {user}");
    }
    let (answered, _) = set(&app, "user-token", 5, "suspended").await;
    assert_eq!(answered, StatusCode::FORBIDDEN, "a user is not an admin");
    for (token, user) in [("admin-token", 6), (TOKEN, 6), (TOKEN, 99)] {
        let (answered, problem) = set(&app, token, user, "suspended").await;
        assert_eq!(answered, StatusCode::NOT_FOUND, "{token} on {user}: {problem}");
    }
    let conn = conn(&temp);
    for user in [1, 2, 3, 5] {
        assert_eq!(status(&conn, user), "active", "{user}");
    }
    assert!(audit(&conn).is_empty());
    assert_eq!(devices(&conn, 2), [(2, None)]);

    let (answered, account) = set(&app, "admin-token", 5, "suspended").await;
    assert_eq!((answered, &account["status"]), (StatusCode::OK, &json!("suspended")));
    let (answered, account) = set(&app, TOKEN, 3, "suspended").await;
    assert_eq!((answered, &account["status"]), (StatusCode::OK, &json!("suspended")));
}

#[tokio::test]
async fn each_change_is_recorded_and_one_that_changes_nothing_does_nothing() {
    let (temp, _state, app) = accounts().await;
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, "user-token").await;
    let conn = conn(&temp);

    // Already active: nothing recorded and no socket closed.
    let (answered, account) = set(&app, TOKEN, 4, "active").await;
    assert_eq!((answered, &account["status"]), (StatusCode::OK, &json!("active")));
    stays_signed_in(&mut socket).await;
    assert!(audit(&conn).is_empty());

    assert_eq!(set(&app, TOKEN, 4, "suspended").await.0, StatusCode::OK);
    assert_eq!(audit(&conn), [("suspended".to_owned(), 1, 4, json!({}))]);
    let (answered, account) = set(&app, TOKEN, 4, "suspended").await;
    assert_eq!((answered, &account["status"]), (StatusCode::OK, &json!("suspended")));
    assert_eq!(audit(&conn).len(), 1);

    assert_eq!(set(&app, TOKEN, 4, "active").await.0, StatusCode::OK);
    assert_eq!(audit(&conn)[1], ("restored".to_owned(), 1, 4, json!({})));
    assert_eq!(set(&app, TOKEN, 4, "active").await.0, StatusCode::OK);
    assert_eq!(audit(&conn).len(), 2);
    assert_eq!(status(&conn, 4), "active");
}

#[tokio::test]
async fn a_suspension_whose_request_is_dropped_mid_write_still_closes_the_sockets() {
    let (_temp, state, app) = accounts().await;
    let address = serve(app.clone()).await;
    let mut phone = connect_with(address, "phone-token").await;

    let (held, release) = hold_next(&state.db, "AFTER UPDATE OF status ON users").await;
    let request = tokio::spawn({
        let app = app.clone();
        async move { set(&app, TOKEN, 4, "suspended").await }
    });
    held.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    drop(release);
    let revoked = json!({ "type": "sessionRevoked", "reason": "logged_out" });
    assert_eq!(next_of(&mut phone, "sessionRevoked").await, revoked);
}

#[tokio::test]
async fn an_admin_suspended_since_the_gate_changes_nothing() {
    let (temp, state, _app) = accounts().await;
    let conn = conn(&temp);
    conn.execute("UPDATE users SET status = 'suspended' WHERE id = 2", []).unwrap();
    let caller = Caller { user: 2, device: 2, admin: true, owner: false };
    let request = StatusChange { status: Status::Suspended };
    let answer = super::set(
        State(state.db.clone()),
        State(state.hub.clone()),
        caller,
        Path(Id(4)),
        extract::Json(request),
    )
    .await;
    let Err(problem) = answer else { panic!("the account was suspended") };
    let problem = serde_json::to_value(problem).unwrap();
    assert_eq!((&problem["status"], &problem["code"]), (&json!(401), &json!("unauthenticated")));
    assert_eq!(status(&conn, 4), "active");
    assert_eq!(devices(&conn, 4)[..2], [(4, None), (7, None)]);
    assert!(audit(&conn).is_empty());
}
