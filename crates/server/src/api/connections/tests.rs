use axum::http::StatusCode;
use serde_json::json;

use super::super::testing::{add_device, add_user, app, call, conn};
use crate::db::testing::{access, library};
use crate::plugins::testing::{connect, enable, install, manifest, plugins};

/// The plugins the caller can connect.
const PLUGINS: &str = "/api/v1/me/plugins";
/// The caller's settings for the scrobbler.
const SETTINGS: &str = "/api/v1/me/plugins/scrobbler/settings";

/// A scrobbler enabled in library 1, which user 2 reaches and has connected and user 3 does
/// not reach, each logged in with a token named after them.
async fn setup() -> (tempfile::TempDir, axum::Router) {
    let (temp, db, app) = app("");
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        library(tx, 2, "Other", "/other")
    })
    .await
    .unwrap();
    let mut scrobbler = manifest("scrobbler", &[]);
    scrobbler["name"] = json!("Scrobbler");
    scrobbler["personalSettings"] = json!({ "properties": {
        "token": { "type": "string", "writeOnly": true }, "user": { "type": "string" }
    } });
    install(&db, scrobbler).await;
    enable(&plugins(&temp, &db), "scrobbler", true).await;
    let conn = conn(&temp);
    for user in [2, 3] {
        add_user(&conn, user, "user");
        add_device(&conn, user, user, &format!("token-{user}"));
    }
    access(&conn, 2, 1).unwrap();
    access(&conn, 3, 2).unwrap();
    connect(&db, "scrobbler", 2, json!({ "token": "t", "user": "two" }), 0).await;
    (temp, app)
}

#[tokio::test]
async fn a_user_sees_and_disconnects_only_what_they_can_reach() {
    let (_temp, app) = setup().await;
    let (status, list) = call(&app, "token-2", "GET", PLUGINS, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        list,
        json!({ "items": [{
            "id": "scrobbler",
            "name": "Scrobbler",
            "connected": true,
            "asksForSearches": false,
            "sharesSearches": false
        }] })
    );

    let (_, settings) = call(&app, "token-2", "GET", SETTINGS, None).await;
    assert_eq!(settings["values"], json!({ "user": "two" }), "the token is never returned");
    assert_eq!(settings["secretsSet"], json!(["token"]));

    assert_eq!(call(&app, "token-3", "GET", PLUGINS, None).await.1, json!({ "items": [] }));
    let (status, _) = call(&app, "token-3", "GET", SETTINGS, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "as if it were not installed");

    let (status, _) = call(&app, "token-2", "DELETE", SETTINGS, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = call(&app, "token-2", "GET", PLUGINS, None).await;
    assert_eq!(list["items"][0]["connected"], json!(false));
}

/// Turns sharing searches with `plugin` on or off as user `user`, and returns the status.
async fn share(app: &axum::Router, user: i64, plugin: &str, sharing: bool) -> StatusCode {
    let uri = format!("/api/v1/me/plugins/{plugin}/search-sharing");
    let body = Some(json!({ "sharing": sharing }));
    call(app, &format!("token-{user}"), "PUT", &uri, body).await.0
}

#[tokio::test]
async fn searches_are_shared_only_with_a_plugin_allowed_them_that_the_user_reaches() {
    let (_temp, app) = setup().await;
    // This scrobbler does not ask for searches.
    assert_eq!(share(&app, 2, "scrobbler", true).await, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        share(&app, 2, "scrobbler", false).await,
        StatusCode::NO_CONTENT,
        "off is always fine"
    );
    assert_eq!(share(&app, 3, "scrobbler", false).await, StatusCode::NOT_FOUND, "out of reach");
    assert_eq!(share(&app, 2, "nothing", true).await, StatusCode::NOT_FOUND);
}
