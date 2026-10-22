use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};

use super::super::testing::{
    add_device, app, app_before_setup, app_with_tracks, eventually, respond, send_json,
};
use crate::db::testing::User;

/// A request with these headers, and its status, headers, and JSON body, if any.
async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let (status, headers, body) = respond(app.clone(), request.body(Body::empty()).unwrap()).await;
    (status, headers, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The owner's token (`testing::TOKEN`) as each kind of credential.
const BEARER: &str = "Bearer owner-token";
const COOKIE: &str = "theme=dark; jewelcase_session=owner-token; other=1";

#[tokio::test]
async fn a_bearer_token_or_the_cookie_identifies_the_caller() {
    let (_temp, _db, app) = app("");
    let owner = json!({ "id": "1", "username": "owner", "displayName": "Owner", "role": "owner",
                        "hasAvatar": false });
    for header in [
        ("authorization", BEARER),
        ("authorization", "bearer owner-token"),
        ("authorization", "Bearer  owner-token"),
        ("cookie", COOKIE),
    ] {
        let (status, _, me) = call(&app, "GET", "/api/v1/me", &[header]).await;
        assert_eq!((status, &me), (StatusCode::OK, &owner), "{header:?}");
    }
    // A bearer token wins, so a stale cookie beside it does not matter.
    let headers = [("authorization", BEARER), ("cookie", "jewelcase_session=stale")];
    assert_eq!(call(&app, "GET", "/api/v1/me", &headers).await.0, StatusCode::OK);
}

#[tokio::test]
async fn another_authorization_scheme_leaves_the_cookie_to_identify_the_caller() {
    // What a browser sends on its own behind a basic-auth or SSO proxy.
    let (_temp, _db, app) = app("");
    // A proxy's header need not be visible ASCII.
    for proxy in ["Basic dXNlcjpwYXNz", "Negotiate", "Digest username=\"Jürgen\""] {
        let headers = [("authorization", proxy), ("cookie", COOKIE)];
        assert_eq!(call(&app, "GET", "/api/v1/me", &headers).await.0, StatusCode::OK, "{proxy}");
        // It is still a cookie request, so a write without the client header is refused.
        let (status, _, _) = call(&app, "POST", "/api/v1/auth/logout", &headers).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{proxy}");
    }
}

#[tokio::test]
async fn a_bad_bearer_token_is_refused_beside_a_good_cookie() {
    let (_temp, _db, app) = app("");
    for bearer in ["Bearer stale", "Bearer\tstale", "Bearer", "bearer "] {
        let headers = [("authorization", bearer), ("cookie", COOKIE)];
        let (status, _, problem) = call(&app, "GET", "/api/v1/me", &headers).await;
        assert_eq!(
            (status, &problem["code"]),
            (StatusCode::UNAUTHORIZED, &json!("unauthenticated")),
            "{bearer}"
        );
    }
}

#[tokio::test]
async fn the_session_from_setup_is_logged_in() {
    let (_temp, _db, app) = app_before_setup("");
    let request = json!({
        "username": "sam",
        "password": "correct horse battery staple",
        "device": { "name": "Phone", "type": "phone" }
    });
    let (_, _, session) = send_json(app.clone(), "POST", "/api/v1/setup", &request).await;
    let bearer = format!("Bearer {}", session["token"].as_str().unwrap());
    let (status, _, me) = call(&app, "GET", "/api/v1/me", &[("authorization", &bearer)]).await;
    assert_eq!((status, &me["username"]), (StatusCode::OK, &json!("sam")));
}

#[tokio::test]
async fn setup_comes_before_authentication() {
    let (_temp, _db, app) = app_before_setup("");
    let (status, _, problem) = call(&app, "GET", "/api/v1/me", &[]).await;
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::SERVICE_UNAVAILABLE, &json!("setup_required"))
    );
}

#[tokio::test]
async fn a_cookie_write_needs_the_client_header() {
    let (_temp, _db, app) = app("");
    let (status, _, problem) =
        call(&app, "POST", "/api/v1/auth/logout", &[("cookie", COOKIE)]).await;
    assert_eq!((status, &problem["code"]), (StatusCode::FORBIDDEN, &json!("forbidden")));
    assert_eq!(call(&app, "GET", "/api/v1/me", &[("cookie", COOKIE)]).await.0, StatusCode::OK);

    let headers = [("cookie", COOKIE), ("x-jewelcase-client", "web")];
    let (status, response, _) = call(&app, "POST", "/api/v1/auth/logout", &headers).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let cookies: Vec<_> = response.get_all(header::SET_COOKIE).iter().collect();
    assert_eq!(cookies, ["jewelcase_session=; Path=/api/v1; Max-Age=0"]);
}

#[tokio::test]
async fn logging_out_removes_the_device_and_its_token() {
    let (temp, db, app) = app("");
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_device(&conn, 1, 2, "other-device");

    // A bearer write needs no client header: no browser sends it on its own.
    let (status, _, _) =
        call(&app, "POST", "/api/v1/auth/logout", &[("authorization", BEARER)]).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        call(&app, "GET", "/api/v1/me", &[("authorization", BEARER)]).await.0,
        StatusCode::UNAUTHORIZED
    );

    let other = [("authorization", "Bearer other-device")];
    assert_eq!(
        call(&app, "GET", "/api/v1/me", &other).await.0,
        StatusCode::OK,
        "other devices stay"
    );
    let ids: Vec<i64> = db
        .read(|conn| {
            conn.prepare("SELECT id FROM devices")?.query_map([], |row| row.get(0))?.collect()
        })
        .await
        .unwrap();
    assert_eq!(ids, [2]);
}

#[tokio::test]
async fn a_suspended_account_is_unauthenticated() {
    let (temp, _db, app) = app("");
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    User { status: "suspended", ..User::new(2, "user") }.insert(&conn).unwrap();
    add_device(&conn, 2, 2, "listener-token");
    let (status, _, problem) =
        call(&app, "GET", "/api/v1/me", &[("authorization", "Bearer listener-token")]).await;
    assert_eq!((status, &problem["code"]), (StatusCode::UNAUTHORIZED, &json!("unauthenticated")));
}

#[tokio::test]
async fn use_refreshes_when_a_device_was_last_seen() {
    let (_temp, db, app) = app("");
    let last_seen = || {
        db.read(|conn| {
            conn.query_row("SELECT last_seen_at FROM devices WHERE id = 1", [], |row| {
                row.get::<_, i64>(0)
            })
        })
    };
    assert_eq!(last_seen().await.unwrap(), 0);
    assert_eq!(
        call(&app, "GET", "/api/v1/me", &[("authorization", BEARER)]).await.0,
        StatusCode::OK
    );
    // The refresh happens beside the request, so it may land a moment later.
    eventually("last_seen_at refreshed", async || (last_seen().await.unwrap() > 0).then_some(()))
        .await;
}

#[tokio::test]
async fn a_bad_token_is_unauthenticated_before_any_library_is_looked_up() {
    let (_temp, _db, app) = app_with_tracks().await;
    let (status, _, _) = call(&app, "GET", "/api/v1/libraries/9/tracks", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
