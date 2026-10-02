use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};

use super::super::testing::{add_device, add_user, app, respond};
use crate::db::libraries;

/// A scrobbler enabled in library 1, which user 2 reaches and user 3 does not, each logged in
/// with a token named after them.
async fn setup() -> (tempfile::TempDir, axum::Router) {
    let (temp, db, app) = app("");
    db.write(|tx| {
        libraries::create(tx, Some(1), "Music", &[Path::new("/music")], &[])?;
        libraries::create(tx, Some(2), "Other", &[Path::new("/other")], &[])?;
        tx.execute_batch(
            r#"INSERT INTO plugins (id, manifest, installed_at, updated_at) VALUES ('scrobbler', '{"id":"scrobbler","name":"Scrobbler","version":"1","apiVersion":"0.2","permissions":[],"personalSettings":{"properties":{"token":{"type":"string","writeOnly":true},"user":{"type":"string"}}}}', 0, 0);
               INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES ('scrobbler', 1, 1);"#,
        )
    })
    .await
    .unwrap();
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    for user in [2, 3] {
        add_user(&conn, user, "user");
        add_device(&conn, user, user, &format!("token-{user}"));
    }
    conn.execute_batch(
        r#"INSERT INTO library_access VALUES (2, 1, 0), (3, 2, 0);
           INSERT INTO plugin_user_settings VALUES ('scrobbler', 2, '{"token":"t","user":"two"}', 0);"#,
    )
    .unwrap();
    (temp, app)
}

async fn call(app: &axum::Router, user: i64, method: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(format!("/api/v1{uri}"))
        .header(header::AUTHORIZATION, format!("Bearer token-{user}"))
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = respond(app.clone(), request).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_user_sees_and_disconnects_only_what_they_can_reach() {
    let (_temp, app) = setup().await;
    let (status, list) = call(&app, 2, "GET", "/me/plugins").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        list,
        json!({ "items": [{ "id": "scrobbler", "name": "Scrobbler", "connected": true }] })
    );

    let (_, settings) = call(&app, 2, "GET", "/me/plugins/scrobbler/settings").await;
    assert_eq!(settings["values"], json!({ "user": "two" }), "the token is never returned");
    assert_eq!(settings["secretsSet"], json!(["token"]));

    assert_eq!(call(&app, 3, "GET", "/me/plugins").await.1, json!({ "items": [] }));
    let (status, _) = call(&app, 3, "GET", "/me/plugins/scrobbler/settings").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "as if it were not installed");

    let (status, _) = call(&app, 2, "DELETE", "/me/plugins/scrobbler/settings").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = call(&app, 2, "GET", "/me/plugins").await;
    assert_eq!(list["items"][0]["connected"], json!(false));
}
