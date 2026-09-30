use axum::http::{HeaderMap, StatusCode, header};
use serde_json::{Value, json};

use super::super::session::token_hash;
use super::super::testing::{app, app_before_setup, json, send_json};

const PASSWORD: &str = "correct horse battery staple";

fn setup_request(username: &str, password: &str) -> Value {
    json!({
        "username": username,
        "password": password,
        "device": { "name": "Firefox on Linux", "type": "desktop", "platform": "web" }
    })
}

fn cookie(headers: &HeaderMap) -> &str {
    headers.get(header::SET_COOKIE).expect("a session cookie").to_str().unwrap()
}

#[tokio::test]
async fn a_new_server_serves_only_setup() {
    let (_temp, _db, app) = app_before_setup("");
    let (status, info) = json(&app, "/api/v1/server").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["setupRequired"], true);
    assert_eq!(info["apiVersion"], "1");
    assert_eq!(json(&app, "/api/v1/health").await.0, StatusCode::OK);

    for uri in ["/api/v1/libraries", "/api/v1/libraries/1/tracks"] {
        let (status, problem) = json(&app, uri).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
        assert_eq!(problem["code"], "setup_required", "{uri}");
    }
}

#[tokio::test]
async fn setup_creates_the_owner_and_logs_it_in() {
    let (_temp, db, app) = app_before_setup("");
    let (status, headers, session) =
        send_json(app.clone(), "POST", "/api/v1/setup", &setup_request("Sam", PASSWORD)).await;
    assert_eq!(status, StatusCode::CREATED, "{session}");

    let token = session["token"].as_str().unwrap().to_owned();
    assert!(token.len() >= 43, "256 bits of base64url: {token}");
    assert_eq!(
        session["user"],
        json!({ "id": session["user"]["id"], "username": "Sam", "displayName": "Sam",
                "role": "owner", "hasAvatar": false })
    );
    let device = &session["device"];
    assert_eq!(
        (&device["name"], &device["type"], &device["platform"], &device["connected"]),
        (&json!("Firefox on Linux"), &json!("desktop"), &json!("web"), &json!(false))
    );
    assert_eq!(device["firstSeenAt"], device["lastSeenAt"]);
    assert_eq!(
        cookie(&headers),
        format!(
            "jewelcase_session={token}; Path=/api/v1; Max-Age=34560000; HttpOnly; SameSite=Strict"
        )
    );

    // Neither the password nor the token is stored readably.
    let (password, stored_hash): (String, Vec<u8>) = db
        .read(|conn| {
            conn.query_row(
                "SELECT password, token_hash FROM users JOIN devices ON devices.user_id = users.id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .await
        .unwrap();
    assert!(password.starts_with("$argon2id$"), "{password}");
    assert_eq!(stored_hash, token_hash(&token));

    assert_eq!(json(&app, "/api/v1/server").await.1["setupRequired"], false);
    assert_eq!(json(&app, "/api/v1/libraries").await.0, StatusCode::OK);
    let (status, _, problem) =
        send_json(app, "POST", "/api/v1/setup", &setup_request("Other", PASSWORD)).await;
    assert_eq!((status, &problem["code"]), (StatusCode::NOT_FOUND, &json!("not_found")));
}

#[tokio::test]
async fn setup_after_restart_is_not_found() {
    let (_temp, _db, app) = app("");
    let (status, _, _) =
        send_json(app, "POST", "/api/v1/setup", &setup_request("Sam", PASSWORD)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn two_setups_at_once_create_one_owner() {
    let (_temp, db, app) = app_before_setup("");
    let (first, second) = (setup_request("First", PASSWORD), setup_request("Second", PASSWORD));
    let ((first, _, _), (second, _, _)) = tokio::join!(
        send_json(app.clone(), "POST", "/api/v1/setup", &first),
        send_json(app, "POST", "/api/v1/setup", &second),
    );
    let mut statuses = [first, second];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::NOT_FOUND]);
    let users: i64 = db
        .read(|conn| conn.query_row("SELECT count(*) FROM users", [], |row| row.get(0)))
        .await
        .unwrap();
    assert_eq!(users, 1);
}

#[tokio::test]
async fn the_cookie_is_scoped_to_the_api_under_the_base_path() {
    let (_temp, _db, app) = app_before_setup("/music");
    let (status, headers, _) =
        send_json(app, "POST", "/music/api/v1/setup", &setup_request("Sam", PASSWORD)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(cookie(&headers).contains("; Path=/music/api/v1;"), "{}", cookie(&headers));
}

#[tokio::test]
async fn an_unacceptable_request_creates_nothing() {
    let (_temp, _db, app) = app_before_setup("");
    let mut bad_device = setup_request("Sam", PASSWORD);
    bad_device["device"]["type"] = json!("toaster");
    let mut blank_name = setup_request("Sam", PASSWORD);
    blank_name["displayName"] = json!("  ");
    let cases = [
        (setup_request("Sam", "hunter22"), "weak_password"),
        (setup_request("jewelcaseowner", "jewelcaseowner"), "weak_password"),
        (setup_request("", PASSWORD), "validation_failed"),
        (setup_request("sam bradley", PASSWORD), "validation_failed"),
        (setup_request(&"s".repeat(33), PASSWORD), "validation_failed"),
        (bad_device, "validation_failed"),
        (blank_name, "validation_failed"),
        (json!({ "username": "Sam", "password": PASSWORD }), "validation_failed"),
    ];
    for (request, code) in cases {
        let (status, _, problem) = send_json(app.clone(), "POST", "/api/v1/setup", &request).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{request}");
        assert_eq!(problem["code"], code, "{request}: {problem}");
    }
    assert_eq!(json(&app, "/api/v1/server").await.1["setupRequired"], true);
}
