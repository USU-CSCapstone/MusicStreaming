//! Setting an account's libraries.

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use futures_util::SinkExt;
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::api::testing::socket::{Socket, connect_with, next, next_of, serve};
use crate::api::testing::{TOKEN, add_device, add_user, app, audit, call, conn, hold_next};
use crate::db::Database;
use crate::db::testing::{access, library, mark_deleting};

/// The API with libraries 1 to 3; the owner (user 1, [`TOKEN`]); admins 2 (`admin-token`) and
/// 3; users 4 (`user-token`), holding libraries 1 and 2, and 5 (`other-token`); and user 6,
/// being deleted.
fn accounts() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app("");
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
    (temp, db, app)
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
    let (temp, _, app) = accounts();
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
    let (temp, _, app) = accounts();
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
    let (temp, _, app) = accounts();
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
    let (temp, _, app) = accounts();
    let (status, problem) = set(&app, TOKEN, 4, &["3", "99"]).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["detail"], "libraryIds names a library that does not exist");
    let conn = conn(&temp);
    assert_eq!(grants(&conn, 4), [(1, 0), (2, 0)]);
    assert!(audit(&conn).is_empty());
}

#[tokio::test]
async fn only_an_account_the_caller_outranks_can_be_set() {
    let (temp, _, app) = accounts();
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

/// `adminChangeLibraryAccess` as `token`, naming `users` and `libraries`.
async fn change(
    app: &Router,
    token: &str,
    users: &[&str],
    libraries: &[&str],
    action: &str,
) -> (StatusCode, Value) {
    let body = json!({ "userIds": users, "libraryIds": libraries, "action": action });
    call(app, token, "POST", "/api/v1/admin/users/library-access", Some(body)).await
}

#[tokio::test]
async fn a_bulk_grant_and_revoke_apply_to_every_account_and_record_each_alone() {
    let (temp, _, app) = accounts();
    let conn = conn(&temp);
    access(&conn, 5, 3).unwrap();
    let address = serve(app.clone()).await;
    let mut four = connect_with(address, "user-token").await;
    let mut five = connect_with(address, "other-token").await;

    // Account 4 holds 1 and 2; account 5 holds 3. Naming 2 and 3 grants 4 only 3, and 5 only 2.
    let (status, body) = change(&app, TOKEN, &["4", "5", "4"], &["3", "2", "3"], "grant").await;
    assert_eq!((status, &body), (StatusCode::NO_CONTENT, &Value::Null));
    assert_eq!(grants(&conn, 4).iter().map(|g| g.0).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(grants(&conn, 5).iter().map(|g| g.0).collect::<Vec<_>>(), [2, 3]);
    let rows = audit(&conn);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        ("access_changed".to_owned(), 1, 4, json!({"granted": ["3"], "revoked": []}))
    );
    assert_eq!(
        rows[1],
        ("access_changed".to_owned(), 1, 5, json!({"granted": ["2"], "revoked": []}))
    );
    assert_eq!(next_of(&mut four, "accessChanged").await, json!({ "type": "accessChanged" }));
    assert_eq!(next_of(&mut five, "accessChanged").await, json!({ "type": "accessChanged" }));

    // Account 4 holds 1 and 3 of those and 5 holds only 3, so both change.
    let (status, _) = change(&app, TOKEN, &["4", "5"], &["1", "3"], "revoke").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(grants(&conn, 4).iter().map(|g| g.0).collect::<Vec<_>>(), [2]);
    assert!(grants(&conn, 5).iter().all(|g| g.0 == 2));
    let rows = audit(&conn);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[2].3, json!({"granted": [], "revoked": ["1", "3"]}));
    assert_eq!(rows[3].3, json!({"granted": [], "revoked": ["3"]}));
    next_of(&mut four, "accessChanged").await;
    next_of(&mut five, "accessChanged").await;

    // Nothing to change: no row and no message.
    let (status, _) = change(&app, TOKEN, &["4", "5"], &["1"], "revoke").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(audit(&conn).len(), 4);
    hears_no_change(&mut four).await;
    hears_no_change(&mut five).await;
}

#[tokio::test]
async fn a_bulk_grant_gives_each_account_liked_songs_for_the_new_library() {
    let (temp, _, app) = accounts();
    let (status, _) = change(&app, TOKEN, &["4", "5"], &["3"], "grant").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let count: i64 = conn(&temp)
        .query_row(
            "SELECT count(*) FROM playlists WHERE user_id IN (4, 5) AND library_id = 3 \
             AND kind = 'liked'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn one_refused_or_unknown_account_or_library_changes_nothing() {
    let (temp, _, app) = accounts();
    let conn = conn(&temp);
    let wrong = [
        ("admin-token", "3", StatusCode::FORBIDDEN),
        ("admin-token", "1", StatusCode::FORBIDDEN),
        ("admin-token", "2", StatusCode::FORBIDDEN),
        ("admin-token", "6", StatusCode::NOT_FOUND),
        ("admin-token", "99", StatusCode::NOT_FOUND),
    ];
    for (token, user, status) in wrong {
        for action in ["grant", "revoke"] {
            let (answered, problem) =
                change(&app, token, &["5", user, "4"], &["1", "3"], action).await;
            assert_eq!(answered, status, "{user} {action}: {problem}");
        }
    }
    let (status, problem) = change(&app, "admin-token", &["4", "3"], &["3"], "grant").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(problem["detail"], "Only the owner can do this to an admin's account.");
    let (status, problem) = change(&app, TOKEN, &["4", "5"], &["3", "99"], "grant").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["detail"], "libraryIds names a library that does not exist");

    assert_eq!(grants(&conn, 4), [(1, 0), (2, 0)]);
    for user in [1, 2, 3, 5, 6] {
        assert!(grants(&conn, user).is_empty(), "{user}");
    }
    assert!(audit(&conn).is_empty());
}

#[tokio::test]
async fn a_non_admin_cannot_change_access_in_bulk() {
    let (temp, _, app) = accounts();
    let (status, _) = change(&app, "user-token", &["5"], &["1"], "grant").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(grants(&conn(&temp), 5).is_empty());
}

#[tokio::test]
async fn naming_no_account_or_over_a_thousand_is_refused() {
    let (temp, _, app) = accounts();
    let (status, problem) = change(&app, TOKEN, &[], &["1"], "grant").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["detail"], "userIds must name at least one account");

    let many: Vec<String> = (0..1001).map(|id| (id + 100).to_string()).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let (status, problem) = change(&app, TOKEN, &many, &["1"], "grant").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["detail"], "userIds names more than 1000 accounts");
    assert!(audit(&conn(&temp)).is_empty());
}

/// Exactly 1,000 accounts is the most a request may name, and each gets its own record.
#[tokio::test]
async fn a_grant_to_a_thousand_accounts_is_accepted() {
    let (temp, _, app) = accounts();
    let conn = conn(&temp);
    for id in 100..1100 {
        add_user(&conn, id, "user");
    }
    let ids: Vec<String> = (100..1100).map(|id| id.to_string()).collect();
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let (status, _) = change(&app, TOKEN, &ids, &["1", "2", "3"], "grant").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(audit(&conn).len(), 1000);
}

#[tokio::test]
async fn a_change_whose_request_is_dropped_mid_write_is_still_announced() {
    let (_temp, db, app) = accounts();
    let address = serve(app.clone()).await;
    let mut target = connect_with(address, "user-token").await;

    let (held, release) = hold_next(&db, "AFTER INSERT ON admin_actions").await;
    let request = tokio::spawn({
        let app = app.clone();
        async move { set(&app, TOKEN, 4, &["1", "2", "3"]).await }
    });
    held.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    drop(release);
    assert_eq!(next_of(&mut target, "accessChanged").await, json!({ "type": "accessChanged" }));

    let (held, release) = hold_next(&db, "AFTER INSERT ON admin_actions").await;
    let request = tokio::spawn({
        let app = app.clone();
        async move { change(&app, TOKEN, &["4"], &["3"], "revoke").await }
    });
    held.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    drop(release);
    assert_eq!(next_of(&mut target, "accessChanged").await, json!({ "type": "accessChanged" }));
}
