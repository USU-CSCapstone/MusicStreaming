//! Listing accounts.

use axum::Router;
use axum::http::StatusCode;
use serde_json::json;

use super::{SELECT, source};
use crate::api::testing::{TOKEN, add_device, add_user, app, call, conn};
use crate::db::now_ms;
use crate::db::testing::{access, library, mark_deleting};

const USERS: &str = "/api/v1/admin/users";

/// The API with libraries 1 to 3 and the owner (user 1, [`TOKEN`]).
fn listed() -> (tempfile::TempDir, Router) {
    let (temp, _db, app) = app("");
    let conn = conn(&temp);
    for id in 1..=3 {
        library(&conn, id, &format!("Library {id}"), temp.path().join(format!("music{id}")))
            .unwrap();
    }
    (temp, app)
}

/// The usernames on the page `query` asks for.
async fn usernames(app: &Router, query: &str) -> Vec<String> {
    let (status, page) = call(app, TOKEN, "GET", &format!("{USERS}?{query}"), None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page["items"].as_array().unwrap().iter().map(|u| u["username"].to_string()).collect()
}

#[tokio::test]
async fn the_list_pages_by_username_with_each_accounts_role_status_and_libraries() {
    let (temp, app) = listed();
    let conn = conn(&temp);
    add_user(&conn, 2, "admin");
    add_user(&conn, 3, "user");
    add_user(&conn, 4, "user");
    add_user(&conn, 5, "user");
    conn.execute("UPDATE users SET username = 'zed' WHERE id = 2", []).unwrap();
    conn.execute("UPDATE users SET username = 'amy', status = 'suspended' WHERE id = 3", [])
        .unwrap();
    conn.execute("UPDATE users SET username = 'Bob', avatar_hash = x'00' WHERE id = 4", [])
        .unwrap();
    for (user, library) in [(3, 3), (3, 1), (4, 2), (2, 2), (2, 1), (2, 3)] {
        access(&conn, user, library).unwrap();
    }

    let mut seen = Vec::new();
    let mut cursor = String::new();
    loop {
        let (status, page) =
            call(&app, TOKEN, "GET", &format!("{USERS}?limit=2{cursor}"), None).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["total"], 5);
        seen.extend(page["items"].as_array().unwrap().clone());
        match page["nextCursor"].as_str() {
            Some(next) => cursor = format!("&cursor={next}"),
            None => break,
        }
    }
    let names: Vec<&str> = seen.iter().map(|u| u["username"].as_str().unwrap()).collect();
    assert_eq!(names, ["amy", "Bob", "owner", "user5", "zed"]);
    let by = |name: &str| seen.iter().find(|u| u["username"] == name).unwrap();
    assert_eq!(
        by("amy"),
        &json!({
            "id": "3", "username": "amy", "displayName": "User 3", "role": "user",
            "hasAvatar": false, "status": "suspended", "createdAt": "1970-01-01T00:00:00.000Z",
            "libraryIds": ["1", "3"], "resetPending": false,
        })
    );
    assert_eq!(by("Bob")["libraryIds"], json!(["2"]));
    assert_eq!(by("Bob")["hasAvatar"], true);
    assert_eq!(by("owner")["role"], "owner");
    assert_eq!(by("owner")["libraryIds"], json!([]));
    assert_eq!(by("zed")["role"], "admin");
    assert_eq!(by("zed")["libraryIds"], json!(["1", "2", "3"]));
}

#[tokio::test]
async fn q_matches_the_username_or_display_name_and_takes_wildcards_literally() {
    let (temp, app) = listed();
    let conn = conn(&temp);
    for (id, username, display) in
        [(2, "ann", "Zoe"), (3, "bo", "ANNE"), (4, "c_d", "plain"), (5, "cxd", "50% off")]
    {
        add_user(&conn, id, "user");
        conn.execute(
            "UPDATE users SET username = ?2, display_name = ?3 WHERE id = ?1",
            rusqlite::params![id, username, display],
        )
        .unwrap();
    }
    assert_eq!(usernames(&app, "q=ANN").await, ["\"ann\"", "\"bo\""]);
    assert_eq!(usernames(&app, "q=ZOE").await, ["\"ann\""]);
    assert_eq!(usernames(&app, "q=_").await, ["\"c_d\""]);
    assert_eq!(usernames(&app, "q=%25").await, ["\"cxd\""]);
    assert_eq!(usernames(&app, "q=c_d").await, ["\"c_d\""]);
    assert_eq!(usernames(&app, "q=").await.len(), 5);
    let (status, page) = call(&app, TOKEN, "GET", &format!("{USERS}?q=ann"), None).await;
    assert_eq!((status, &page["total"]), (StatusCode::OK, &json!(2)));
}

#[tokio::test]
async fn an_unknown_parameter_is_refused() {
    let (_temp, app) = listed();
    let (status, _) = call(&app, TOKEN, "GET", &format!("{USERS}?sort=name"), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_reset_is_pending_only_while_unexpired() {
    let (temp, app) = listed();
    let conn = conn(&temp);
    for id in [2, 3, 4] {
        add_user(&conn, id, "user");
    }
    let now = now_ms();
    for (id, hash, expires) in [(2, "a", now + 60_000), (3, "b", now - 1)] {
        conn.execute(
            "UPDATE users SET reset_hash = ?2, reset_expires_at = ?3 WHERE id = ?1",
            rusqlite::params![id, hash.as_bytes(), expires],
        )
        .unwrap();
    }
    let (_, page) = call(&app, TOKEN, "GET", USERS, None).await;
    let pending: Vec<bool> =
        page["items"].as_array().unwrap().iter().map(|u| u["resetPending"] == true).collect();
    // owner, user2, user3, user4
    assert_eq!(pending, [false, true, false, false]);
}

#[tokio::test]
async fn an_account_being_deleted_is_neither_listed_nor_counted() {
    let (temp, app) = listed();
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    add_user(&conn, 3, "user");
    mark_deleting(&conn, 2).unwrap();
    let (_, page) = call(&app, TOKEN, "GET", USERS, None).await;
    assert_eq!(page["total"], 2);
    assert_eq!(usernames(&app, "").await, ["\"owner\"", "\"user3\""]);
}

#[tokio::test]
async fn a_page_reads_libraries_by_the_primary_key() {
    let (temp, _app) = listed();
    let conn = conn(&temp);
    let plan: Vec<String> = conn
        .prepare(&format!(
            "EXPLAIN QUERY PLAN SELECT {} FROM {} WHERE {}",
            SELECT.join(", "),
            source(None).from,
            source(None).filter
        ))
        .unwrap()
        .query_map([], |row| row.get(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(
        plan.iter().any(|step| step.starts_with("SEARCH library_access USING PRIMARY KEY")),
        "{plan:#?}"
    );
    assert!(!plan.iter().any(|step| step.starts_with("SCAN library_access")), "{plan:#?}");
}

#[tokio::test]
async fn a_user_is_refused() {
    let (temp, app) = listed();
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    let (status, _) = call(&app, "user-token", "GET", USERS, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
