//! Setting an account's libraries.

use axum::Router;
use axum::http::StatusCode;
use futures_util::SinkExt;
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::api::testing::socket::{Socket, connect_with, next, next_of, serve};
use crate::api::testing::{TOKEN, add_device, add_user, app, call, conn};
use crate::db::testing::{access, library, mark_deleting};

/// The API with libraries 1 to 3; the owner (user 1, [`TOKEN`]); admins 2 (`admin-token`) and
/// 3; users 4 (`user-token`), holding libraries 1 and 2, and 5 (`other-token`); and user 6,
/// being deleted.
fn accounts() -> (tempfile::TempDir, Router) {
    let (temp, _db, app) = app("");
    let conn = conn(&temp);
    for id in 1..=3 {
        library(&conn, id, &format!("Library {id}"), temp.path().join(format!("music{id}")))
            .unwrap();
    }
    for (id, role) in [(2, "admin"), (3, "admin"), (4, "user"), (5, "user"), (6, "user")] {
        add_user(&conn, id, role);
    }
    add_device(&conn, 2, 2, "admin-token");
    add_device(&conn, 4, 4, "user-token");
    add_device(&conn, 5, 5, "other-token");
    access(&conn, 4, 1).unwrap();
    access(&conn, 4, 2).unwrap();
    mark_deleting(&conn, 6).unwrap();
    (temp, app)
}

/// `adminSetUserLibraries` on account `user` as `token`, setting `libraries`.
async fn set(app: &Router, token: &str, user: i64, libraries: &[&str]) -> (StatusCode, Value) {
    let body = json!({ "libraryIds": libraries });
    call(app, token, "PUT", &format!("/api/v1/admin/users/{user}/libraries"), Some(body)).await
}

/// Account `user`'s grants, ascending, each with when it was granted.
fn grants(conn: &Connection, user: i64) -> Vec<(i64, i64)> {
    conn.prepare("SELECT library_id, granted_at FROM library_access WHERE user_id = ?1 ORDER BY 1")
        .unwrap()
        .query_map([user], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// Every audit row, as (action, actor, target, detail).
fn audit(conn: &Connection) -> Vec<(String, i64, i64, Value)> {
    conn.prepare("SELECT action, actor_id, target_id, detail FROM admin_actions ORDER BY id")
        .unwrap()
        .query_map([], |row| {
            let detail: String = row.get(3)?;
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, serde_json::from_str(&detail).unwrap()))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
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
async fn setting_grants_replaces_them_records_what_changed_and_answers_the_account() {
    let (temp, app) = accounts();
    let (status, account) = set(&app, TOKEN, 4, &["3", "2", "3"]).await;
    assert_eq!(status, StatusCode::OK, "{account}");
    assert_eq!((&account["id"], &account["username"]), (&json!("4"), &json!("user4")));
    assert_eq!(account["libraryIds"], json!(["2", "3"]));

    let conn = conn(&temp);
    let grants = grants(&conn, 4);
    assert_eq!(grants[0], (2, 0), "an existing grant keeps its time");
    assert_eq!(grants[1].0, 3);
    assert!(grants[1].1 > 0, "a new grant is timed now");
    assert_eq!(grants.len(), 2);
    let changed = json!({ "granted": ["3"], "revoked": ["1"] });
    assert_eq!(audit(&conn), [("access_changed".to_owned(), 1, 4, changed)]);

    let (status, account) = set(&app, TOKEN, 4, &[]).await;
    assert_eq!((status, &account["libraryIds"]), (StatusCode::OK, &json!([])));
    let emptied = json!({ "granted": [], "revoked": ["2", "3"] });
    assert_eq!(audit(&conn)[1], ("access_changed".to_owned(), 1, 4, emptied));
}

#[tokio::test]
async fn a_newly_granted_library_has_the_accounts_liked_songs() {
    let (temp, app) = accounts();
    let conn = conn(&temp);
    let liked = |library: i64| -> i64 {
        conn.query_row(
            "SELECT count(*) FROM playlists WHERE user_id = 4 AND library_id = ?1 \
             AND kind = 'liked'",
            [library],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(liked(3), 0);
    let (status, _) = set(&app, TOKEN, 4, &["1", "2", "3"]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(liked(3), 1);
}

#[tokio::test]
async fn the_account_alone_hears_access_changed_and_only_when_something_changed() {
    let (temp, app) = accounts();
    let address = serve(app.clone()).await;
    let mut target = connect_with(address, "user-token").await;
    let mut other = connect_with(address, "other-token").await;

    let (status, _) = set(&app, TOKEN, 4, &["1"]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(next_of(&mut target, "accessChanged").await, json!({ "type": "accessChanged" }));
    hears_no_change(&mut other).await;

    // Nothing changes: no record and no message.
    let (status, account) = set(&app, TOKEN, 4, &["1", "1"]).await;
    assert_eq!((status, &account["libraryIds"]), (StatusCode::OK, &json!(["1"])));
    hears_no_change(&mut target).await;
    assert_eq!(audit(&conn(&temp)).len(), 1);
}

#[tokio::test]
async fn an_unknown_library_changes_nothing() {
    let (temp, app) = accounts();
    let (status, problem) = set(&app, TOKEN, 4, &["3", "99"]).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["detail"], "libraryIds names a library that does not exist");
    let conn = conn(&temp);
    assert_eq!(grants(&conn, 4), [(1, 0), (2, 0)]);
    assert!(audit(&conn).is_empty());
}

#[tokio::test]
async fn only_an_account_the_caller_outranks_can_be_set() {
    let (temp, app) = accounts();
    for (token, user, status) in [
        ("user-token", 5, StatusCode::FORBIDDEN),
        ("admin-token", 3, StatusCode::FORBIDDEN),
        ("admin-token", 1, StatusCode::FORBIDDEN),
        ("admin-token", 2, StatusCode::FORBIDDEN),
        ("admin-token", 6, StatusCode::NOT_FOUND),
        ("admin-token", 99, StatusCode::NOT_FOUND),
        (TOKEN, 99, StatusCode::NOT_FOUND),
    ] {
        let (answered, problem) = set(&app, token, user, &["3"]).await;
        assert_eq!(answered, status, "{token} on {user}: {problem}");
    }
    let conn = conn(&temp);
    assert!(audit(&conn).is_empty());
    for user in [1, 2, 3, 5, 6] {
        assert!(grants(&conn, user).is_empty(), "{user}");
    }

    let (status, problem) = set(&app, "admin-token", 3, &["3"]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(problem["detail"], "Only the owner can do this to an admin's account.");
    let (status, account) = set(&app, "admin-token", 4, &["3"]).await;
    assert_eq!((status, &account["libraryIds"]), (StatusCode::OK, &json!(["3"])));
    let (status, account) = set(&app, TOKEN, 2, &["3"]).await;
    assert_eq!((status, &account["libraryIds"]), (StatusCode::OK, &json!(["3"])));
}
