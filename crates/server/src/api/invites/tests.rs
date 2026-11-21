//! Creating, listing and revoking invites.

use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, header};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::api::session::token_hash;
use crate::api::testing::{TOKEN, add_device, add_user, app, call, conn, owner_request, respond};
use crate::db::now_ms;
use crate::db::testing::{library, mark_deleting};

const INVITES: &str = "/api/v1/admin/invites";

/// The API with libraries 1 and 2, the owner (user 1, [`TOKEN`]) and a `user`-role account
/// (user 2, `user-token`).
fn invited() -> (tempfile::TempDir, Router) {
    let (temp, _db, app) = app("");
    let conn = conn(&temp);
    for id in [1, 2] {
        library(&conn, id, &format!("Library {id}"), temp.path().join(format!("music{id}")))
            .unwrap();
    }
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    (temp, app)
}

/// Stores an invite made by user 1 with the given times, as a hand-made row.
fn insert(conn: &Connection, id: i64, created_at: i64, expires_at: i64) {
    conn.execute(
        "INSERT INTO invites (id, code_hash, created_by, created_at, expires_at) \
         VALUES (?1, ?2, 1, ?3, ?4)",
        params![id, token_hash(&id.to_string()).to_vec(), created_at, expires_at],
    )
    .unwrap();
}

#[tokio::test]
async fn a_created_invite_answers_its_code_once_and_stores_only_its_hash() {
    let (temp, app) = invited();
    let request = owner_request("POST", INVITES)
        .header(header::CONTENT_TYPE, "application/json")
        .header("Idempotency-Key", "abc")
        .body(Body::from(json!({ "libraryIds": ["1", "2"] }).to_string()))
        .unwrap();
    let (status, _, body) = respond(app.clone(), request).await;
    assert_eq!(status, StatusCode::CREATED);
    let created: Value = serde_json::from_slice(&body).unwrap();
    let code = created["code"].as_str().unwrap();
    assert_eq!(created["status"], "pending");
    assert_eq!(created["createdBy"], json!({ "id": "1", "username": "owner" }));
    assert_eq!(created["redeemedBy"], Value::Null);
    assert_eq!(created["libraryIds"], json!(["1", "2"]));
    assert!(created.get("redeemedAt").is_none());

    let conn = conn(&temp);
    let lifetime: i64 =
        conn.query_row("SELECT expires_at - created_at FROM invites", [], |r| r.get(0)).unwrap();
    assert_eq!(lifetime, 7 * 86_400_000);
    let stored: Vec<u8> =
        conn.query_row("SELECT code_hash FROM invites", [], |r| r.get(0)).unwrap();
    assert_eq!(stored, token_hash(code));
    let in_database: i64 = conn
        .query_row(
            "SELECT count(*) FROM invites WHERE instr(CAST(code_hash AS TEXT), ?1) > 0",
            [code],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(in_database, 0);
    let keys: i64 =
        conn.query_row("SELECT count(*) FROM idempotency_keys", [], |r| r.get(0)).unwrap();
    assert_eq!(keys, 0);

    let (_, page) = call(&app, TOKEN, "GET", INVITES, None).await;
    assert!(!page.to_string().contains(code));
}

#[tokio::test]
async fn an_invite_for_an_unknown_library_or_a_bad_lifetime_is_refused() {
    let (temp, app) = invited();
    for body in [
        json!({ "libraryIds": ["1", "99"] }),
        json!({ "libraryIds": ["nope"] }),
        json!({ "libraryIds": [], "expiresInDays": 0 }),
        json!({ "libraryIds": [], "expiresInDays": 31 }),
    ] {
        let (status, _) = call(&app, TOKEN, "POST", INVITES, Some(body.clone())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    let conn = conn(&temp);
    for table in ["invites", "invite_libraries"] {
        let rows: i64 =
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 0, "{table}");
    }
    let (status, empty) =
        call(&app, TOKEN, "POST", INVITES, Some(json!({ "libraryIds": [], "expiresInDays": 30 })))
            .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(empty["libraryIds"], json!([]));
    let lifetime: i64 =
        conn.query_row("SELECT expires_at - created_at FROM invites", [], |r| r.get(0)).unwrap();
    assert_eq!(lifetime, 30 * 86_400_000);
}

#[tokio::test]
async fn a_library_named_twice_is_granted_once() {
    let (_temp, app) = invited();
    let body = json!({ "libraryIds": ["2", "2", "1"] });
    let (status, invite) = call(&app, TOKEN, "POST", INVITES, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{invite}");
    assert_eq!(invite["libraryIds"], json!(["1", "2"]));
}

#[tokio::test]
async fn the_list_shows_each_state_newest_first_across_pages() {
    let (temp, app) = invited();
    let now = now_ms();
    let conn = conn(&temp);
    insert(&conn, 10, 1000, now + 100_000);
    insert(&conn, 20, 2000, now + 100_000);
    insert(&conn, 30, 3000, now + 100_000);
    insert(&conn, 40, 4000, now - 1);
    // Two created at once come in descending ID order.
    insert(&conn, 50, 4000, now + 100_000);
    conn.execute("UPDATE invites SET redeemed_by = 2, redeemed_at = 5 WHERE id = 20", []).unwrap();
    conn.execute("UPDATE invites SET revoked_at = 5 WHERE id = 30", []).unwrap();
    conn.execute("INSERT INTO invite_libraries VALUES (10, 1), (10, 2)", []).unwrap();

    let mut seen = Vec::new();
    let mut cursor = String::new();
    loop {
        let (status, page) =
            call(&app, TOKEN, "GET", &format!("{INVITES}?limit=2{cursor}"), None).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["total"], 5);
        for item in page["items"].as_array().unwrap() {
            seen.push((
                item["id"].as_str().unwrap().to_owned(),
                item["status"].as_str().unwrap().to_owned(),
            ));
            if item["id"] == "20" {
                assert_eq!(item["redeemedBy"], json!({ "id": "2", "username": "user2" }));
                assert!(item["redeemedAt"].is_string());
            }
            if item["id"] == "10" {
                assert_eq!(item["libraryIds"], json!(["1", "2"]));
            }
        }
        match page["nextCursor"].as_str() {
            Some(next) => cursor = format!("&cursor={next}"),
            None => break,
        }
    }
    let expected = [
        ("50", "pending"),
        ("40", "expired"),
        ("30", "revoked"),
        ("20", "redeemed"),
        ("10", "pending"),
    ];
    assert_eq!(seen, expected.map(|(id, status)| (id.to_owned(), status.to_owned())));
}

#[tokio::test]
async fn an_invite_whose_creator_is_being_deleted_lists_with_no_creator() {
    let (temp, app) = invited();
    let conn = conn(&temp);
    insert(&conn, 10, 1000, now_ms() + 100_000);
    conn.execute("UPDATE invites SET created_by = 2 WHERE id = 10", []).unwrap();
    mark_deleting(&conn, 2).unwrap();
    let (_, page) = call(&app, TOKEN, "GET", INVITES, None).await;
    assert_eq!(page["items"][0]["createdBy"], Value::Null);
}

#[tokio::test]
async fn revoking_answers_the_invite_as_it_now_is() {
    let (temp, app) = invited();
    let now = now_ms();
    let conn = conn(&temp);
    insert(&conn, 10, 1000, now + 100_000);
    insert(&conn, 20, 2000, now + 100_000);
    insert(&conn, 30, 3000, now - 1);
    conn.execute("UPDATE invites SET redeemed_by = 2, redeemed_at = 5 WHERE id = 20", []).unwrap();

    for (id, status) in [(10, "revoked"), (20, "redeemed"), (30, "expired")] {
        let (code, invite) = call(&app, TOKEN, "DELETE", &format!("{INVITES}/{id}"), None).await;
        assert_eq!(code, StatusCode::OK, "{invite}");
        assert_eq!(invite["status"], status, "{id}");
    }
    // Revoking again answers revoked and leaves the time it was revoked at.
    conn.execute("UPDATE invites SET revoked_at = 7 WHERE id = 10", []).unwrap();
    let (_, again) = call(&app, TOKEN, "DELETE", &format!("{INVITES}/10"), None).await;
    assert_eq!(again["status"], "revoked");
    let at: i64 =
        conn.query_row("SELECT revoked_at FROM invites WHERE id = 10", [], |r| r.get(0)).unwrap();
    assert_eq!(at, 7);
    let revoked: Vec<i64> = conn
        .prepare("SELECT id FROM invites WHERE revoked_at IS NOT NULL")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(revoked, [10]);
    let (status, _) = call(&app, TOKEN, "DELETE", &format!("{INVITES}/99"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_user_is_refused_all_three() {
    let (_temp, app) = invited();
    let create = Some(json!({ "libraryIds": [] }));
    for (method, uri, body) in [
        ("POST", INVITES.to_owned(), create),
        ("GET", INVITES.to_owned(), None),
        ("DELETE", format!("{INVITES}/1"), None),
    ] {
        let (status, _) = call(&app, "user-token", method, &uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}");
    }
}
