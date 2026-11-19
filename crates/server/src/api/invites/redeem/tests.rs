//! Checking and redeeming invites.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::api::session::token_hash;
use crate::api::testing::{TOKEN, add_user, app, app_before_setup, call, conn, respond, send_json};
use crate::db::now_ms;
use crate::db::testing::{library, mark_deleting};

const CHECK: &str = "/api/v1/auth/invites/check";
const REDEEM: &str = "/api/v1/auth/invites/redeem";
const PASSWORD: &str = "correct horse battery staple";

/// The API with libraries 1, 2 and 3 and the owner (user 1, [`TOKEN`]).
fn server() -> (tempfile::TempDir, Router) {
    let (temp, _db, app) = app("");
    let conn = conn(&temp);
    for id in [1, 2, 3] {
        library(&conn, id, &format!("Library {id}"), temp.path().join(format!("music{id}")))
            .unwrap();
    }
    (temp, app)
}

/// A pending invite to `libraries`, made by the owner through the API, and its code.
async fn invite(app: &Router, libraries: &[&str]) -> (String, String) {
    let body = json!({ "libraryIds": libraries });
    let (status, created) = call(app, TOKEN, "POST", "/api/v1/admin/invites", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    (created["id"].as_str().unwrap().to_owned(), created["code"].as_str().unwrap().to_owned())
}

/// Stores invite `id` with code `code`, made by `creator` and expiring at `expires_at`.
fn insert(conn: &Connection, id: i64, code: &str, creator: i64, expires_at: i64) {
    conn.execute(
        "INSERT INTO invites (id, code_hash, created_by, created_at, expires_at) \
         VALUES (?1, ?2, ?3, 0, ?4)",
        params![id, token_hash(code).to_vec(), creator, expires_at],
    )
    .unwrap();
}

/// A `redeemInvite` body for `code` and `username`, on a desktop browser.
fn redeem_request(code: &str, username: &str, password: &str) -> Value {
    json!({
        "code": code,
        "username": username,
        "password": password,
        "device": { "name": "Firefox on Linux", "type": "desktop", "platform": "web" }
    })
}

/// How many accounts there are with the `user` role.
fn users(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM users WHERE role = 'user'", [], |r| r.get(0)).unwrap()
}

/// The invite `id` as the admins' list shows it.
async fn listed(app: &Router, id: &str) -> Value {
    let (_, page) = call(app, TOKEN, "GET", "/api/v1/admin/invites", None).await;
    page["items"].as_array().unwrap().iter().find(|item| item["id"] == id).unwrap().clone()
}

#[tokio::test]
async fn redeeming_makes_a_user_with_the_invites_libraries_and_signs_it_in() {
    let (temp, app) = server();
    let (id, code) = invite(&app, &["1", "3"]).await;
    // Another invite's library must not reach this account.
    invite(&app, &["2"]).await;
    let check = Request::post(CHECK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "code": code }).to_string()));
    let (status, _, body) = respond(app.clone(), check.unwrap()).await;
    assert_eq!((status, body.is_empty()), (StatusCode::NO_CONTENT, true));

    let mut request = redeem_request(&code, "Alex", PASSWORD);
    request["displayName"] = json!("Alex B");
    let (status, headers, session) = send_json(app.clone(), "POST", REDEEM, &request).await;
    assert_eq!(status, StatusCode::CREATED, "{session}");
    let user = session["user"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        session["user"],
        json!({ "id": user, "username": "Alex", "displayName": "Alex B", "role": "user",
                "hasAvatar": false })
    );
    let token = session["token"].as_str().unwrap();
    let cookie = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(cookie.starts_with(&format!("jewelcase_session={token};")), "{cookie}");
    let (status, me) = call(&app, token, "GET", "/api/v1/me", None).await;
    assert_eq!((status, &me["username"]), (StatusCode::OK, &json!("Alex")));

    let conn = conn(&temp);
    let reached = |sql: &str| -> Vec<i64> {
        conn.prepare(sql)
            .unwrap()
            .query_map([user.parse::<i64>().unwrap()], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(
        reached("SELECT library_id FROM library_access WHERE user_id = ?1 ORDER BY library_id"),
        [1, 3]
    );
    assert_eq!(
        reached(
            "SELECT library_id FROM playlists WHERE user_id = ?1 AND kind = 'liked' \
             ORDER BY library_id"
        ),
        [1, 3]
    );
    assert_eq!(reached("SELECT succeeded FROM login_attempts WHERE user_id = ?1"), [1]);

    let invite = listed(&app, &id).await;
    assert_eq!(invite["status"], "redeemed");
    assert_eq!(invite["redeemedBy"], json!({ "id": user, "username": "Alex" }));

    let audit: (String, i64, String, String, String, String) = conn
        .query_row(
            "SELECT action, actor_id, actor_name, target_id, target_name, detail \
             FROM admin_actions",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get::<_, i64>(3)?.to_string(),
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .unwrap();
    let (action, actor, actor_name, target, target_name, detail) = audit;
    assert_eq!((action.as_str(), actor, actor_name.as_str()), ("account_created", 1, "owner"));
    assert_eq!((target, target_name.as_str()), (user, "Alex"));
    assert_eq!(
        serde_json::from_str::<Value>(&detail).unwrap(),
        json!({ "inviteId": id, "libraryIds": ["1", "3"] })
    );
}

#[tokio::test]
async fn two_redemptions_at_once_make_one_account() {
    let (temp, app) = server();
    let (_, code) = invite(&app, &["1"]).await;
    let (first, second) =
        (redeem_request(&code, "first", PASSWORD), redeem_request(&code, "second", PASSWORD));
    let ((first, _, _), (second, _, _)) = tokio::join!(
        send_json(app.clone(), "POST", REDEEM, &first),
        send_json(app, "POST", REDEEM, &second),
    );
    let mut statuses = [first, second];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::NOT_FOUND]);
    assert_eq!(users(&conn(&temp)), 1);
}

/// Every code that is not live answers the same `404` from both operations, before the rest of
/// the request is looked at: a weak password or a bad name would otherwise be `422`.
#[tokio::test]
async fn a_dead_code_is_refused_alike_before_anything_else() {
    let (temp, app) = server();
    let conn = conn(&temp);
    let now = now_ms();
    insert(&conn, 10, "redeemed", 1, now + 100_000);
    insert(&conn, 20, "revoked", 1, now + 100_000);
    // Expired, though no sweep has run.
    insert(&conn, 30, "expired", 1, now - 1);
    conn.execute("UPDATE invites SET redeemed_at = 5 WHERE id = 10", []).unwrap();
    conn.execute("UPDATE invites SET revoked_at = 5 WHERE id = 20", []).unwrap();

    let (_, _, unknown) = send_json(app.clone(), "POST", CHECK, &json!({ "code": "nope" })).await;
    assert_eq!(unknown["code"], "not_found");
    for code in ["nope", "redeemed", "revoked", "expired"] {
        let (status, _, problem) =
            send_json(app.clone(), "POST", CHECK, &json!({ "code": code })).await;
        assert_eq!((status, &problem), (StatusCode::NOT_FOUND, &unknown), "{code}");
        for request in [
            redeem_request(code, "Alex", PASSWORD),
            redeem_request(code, "Alex", "hunter22"),
            redeem_request(code, "not a name", PASSWORD),
        ] {
            let (status, _, problem) = send_json(app.clone(), "POST", REDEEM, &request).await;
            assert_eq!((status, &problem), (StatusCode::NOT_FOUND, &unknown), "{request}");
        }
    }
    assert_eq!(users(&conn), 0);
}

#[tokio::test]
async fn a_taken_username_is_refused_only_with_a_live_code() {
    let (temp, app) = server();
    let (id, code) = invite(&app, &["1"]).await;
    let (status, _, problem) =
        send_json(app.clone(), "POST", REDEEM, &redeem_request(&code, "OWNER", PASSWORD)).await;
    assert_eq!((status, &problem["code"]), (StatusCode::CONFLICT, &json!("username_taken")));
    assert_eq!(problem["detail"], "That username is taken.");
    let conn = conn(&temp);
    assert_eq!(users(&conn), 0);
    let invite = listed(&app, &id).await;
    assert_eq!((&invite["status"], &invite["redeemedBy"]), (&json!("pending"), &Value::Null));
    let rows: i64 = conn.query_row("SELECT count(*) FROM admin_actions", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 0);

    let (status, _, _) =
        send_json(app, "POST", REDEEM, &redeem_request("nope", "OWNER", PASSWORD)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_inviter_being_deleted_is_recorded_by_id_without_a_name() {
    let (temp, app) = server();
    let conn = conn(&temp);
    add_user(&conn, 2, "admin");
    insert(&conn, 10, "code", 2, now_ms() + 100_000);
    mark_deleting(&conn, 2).unwrap();
    let (status, _, session) =
        send_json(app, "POST", REDEEM, &redeem_request("code", "Alex", PASSWORD)).await;
    assert_eq!(status, StatusCode::CREATED, "{session}");
    let (actor, actor_name, target_name): (i64, Option<String>, String) = conn
        .query_row("SELECT actor_id, actor_name, target_name FROM admin_actions", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!((actor, actor_name, target_name.as_str()), (2, None, "Alex"));
}

#[tokio::test]
async fn both_wait_for_setup() {
    let (_temp, _db, app) = app_before_setup("");
    for (uri, body) in
        [(CHECK, json!({ "code": "c" })), (REDEEM, redeem_request("c", "Alex", PASSWORD))]
    {
        let (status, _, problem) = send_json(app.clone(), "POST", uri, &body).await;
        assert_eq!(
            (status, &problem["code"]),
            (StatusCode::SERVICE_UNAVAILABLE, &json!("setup_required")),
            "{uri}"
        );
    }
}
