//! Reading the record.

use axum::Router;
use axum::http::StatusCode;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::api::testing::{TOKEN, add_device, add_user, app, call, conn};
use crate::db::testing::{User, library, mark_deleting};

const LOG: &str = "/api/v1/admin/audit-log";

/// The API with library 1, the owner (user 1, [`TOKEN`]), admin 2 (`admin-token`) and users 3
/// (`user-token`) and 4.
fn logged() -> (tempfile::TempDir, Router) {
    let (temp, _db, app) = app("");
    let conn = conn(&temp);
    library(&conn, 1, "Library 1", temp.path().join("music")).unwrap();
    for (id, role) in [(2, "admin"), (3, "user"), (4, "user")] {
        add_user(&conn, id, role);
    }
    add_device(&conn, 2, 2, "admin-token");
    add_device(&conn, 3, 3, "user-token");
    (temp, app)
}

/// Stores a row by hand, with the names it is given.
fn insert(
    conn: &Connection,
    action: &str,
    actor: Option<(i64, Option<&str>)>,
    target: (i64, &str),
    detail: &Value,
) {
    conn.execute(
        "INSERT INTO admin_actions (at, action, actor_id, actor_name, target_id, target_name, detail) \
         VALUES (86400000, ?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            action,
            actor.map(|(id, _)| id),
            actor.and_then(|(_, name)| name),
            target.0,
            target.1,
            detail.to_string()
        ],
    )
    .unwrap();
}

#[tokio::test]
async fn entries_come_newest_first_across_pages_with_each_actions_detail() {
    let (temp, app) = logged();
    let conn = conn(&temp);
    let created = json!({ "inviteId": "9", "libraryIds": ["1"] });
    insert(&conn, "account_created", Some((1, Some("owner"))), (3, "user3"), &created);
    let (status, _) = call(
        &app,
        TOKEN,
        "PUT",
        "/api/v1/admin/users/2/libraries",
        Some(json!({ "libraryIds": ["1"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    insert(&conn, "suspended", Some((2, Some("user2"))), (4, "user4"), &json!({}));

    let (status, first) = call(&app, TOKEN, "GET", &format!("{LOG}?limit=2"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["total"], 3);
    let actions = |page: &Value| -> Vec<String> {
        page["items"].as_array().unwrap().iter().map(|e| e["action"].to_string()).collect()
    };
    assert_eq!(actions(&first), [r#""suspended""#, r#""access_changed""#]);
    let cursor = first["nextCursor"].as_str().unwrap();
    let (_, second) =
        call(&app, TOKEN, "GET", &format!("{LOG}?limit=2&cursor={cursor}"), None).await;
    assert_eq!(actions(&second), [r#""account_created""#]);
    assert_eq!(second["nextCursor"], Value::Null);

    let changed = &first["items"][1];
    assert_eq!(changed["actor"], json!({ "id": "1", "username": "owner", "deleted": false }));
    assert_eq!(changed["target"], json!({ "id": "2", "username": "user2", "deleted": false }));
    assert_eq!(changed["detail"], json!({ "granted": ["1"], "revoked": [] }));
    assert_eq!(second["items"][0]["detail"], created);
    assert_eq!(first["items"][0]["detail"], json!({}));
    assert_eq!(second["items"][0]["at"], "1970-01-02T00:00:00.000Z");
}

#[tokio::test]
async fn an_account_since_deleting_shows_its_stored_name_as_deleted() {
    let (temp, app) = logged();
    let conn = conn(&temp);
    insert(&conn, "suspended", Some((1, Some("owner"))), (3, "user3"), &json!({}));
    mark_deleting(&conn, 3).unwrap();
    let (_, page) = call(&app, TOKEN, "GET", LOG, None).await;
    assert_eq!(
        page["items"][0]["target"],
        json!({ "id": "3", "username": "user3", "deleted": true })
    );
    assert_eq!(page["items"][0]["actor"]["deleted"], false);
}

#[tokio::test]
async fn a_name_taken_later_by_a_new_account_does_not_point_at_it() {
    let (temp, app) = logged();
    let conn = conn(&temp);
    insert(&conn, "suspended", Some((1, Some("owner"))), (3, "user3"), &json!({}));
    mark_deleting(&conn, 3).unwrap();
    User::new(5, "user").insert(&conn).unwrap();
    conn.execute("UPDATE users SET username = 'user3' WHERE id = 5", []).unwrap();
    let (_, page) = call(&app, TOKEN, "GET", LOG, None).await;
    assert_eq!(
        page["items"][0]["target"],
        json!({ "id": "3", "username": "user3", "deleted": true })
    );
    assert!(!page["items"][0].to_string().contains(r#""id":"5""#));
}

#[tokio::test]
async fn an_entry_no_one_made_has_no_actor_and_one_whose_actor_had_no_name_has_a_null_name() {
    let (temp, app) = logged();
    let conn = conn(&temp);
    insert(&conn, "password_reset_used", None, (3, "user3"), &json!({}));
    insert(&conn, "deleted", Some((4, None)), (3, "user3"), &json!({}));
    mark_deleting(&conn, 4).unwrap();
    let (_, page) = call(&app, TOKEN, "GET", LOG, None).await;
    assert_eq!(page["items"][0]["actor"], json!({ "id": "4", "username": null, "deleted": true }));
    assert_eq!(page["items"][1]["actor"], Value::Null);
}

#[tokio::test]
async fn only_the_owner_reads_the_log() {
    let (_temp, app) = logged();
    let (status, problem) = call(&app, "admin-token", "GET", LOG, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(problem["detail"], "Only the owner can read the audit log.");
    let (status, _) = call(&app, "user-token", "GET", LOG, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, page) = call(&app, TOKEN, "GET", LOG, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"], json!([]));
}

#[tokio::test]
async fn an_unknown_parameter_is_refused() {
    let (_temp, app) = logged();
    let (status, _) = call(&app, TOKEN, "GET", &format!("{LOG}?action=deleted"), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
