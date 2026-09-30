use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};

use super::super::testing::{add_device, add_user, app_with_tracks, respond};

const ADMIN: &str = "Bearer owner-token";

/// Sends a request as `token` with a JSON body, if any, and returns the status and JSON body.
async fn call(
    app: &Router,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri).header(header::AUTHORIZATION, token);
    let request = match body {
        Some(body) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    };
    let (status, _, body) = respond(app.clone(), request.unwrap()).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// Libraries 1 and 2, with `lrclib-lyrics` installed as a row: what a successful install
/// leaves, without needing a compiled component.
async fn server() -> (tempfile::TempDir, Router) {
    let (temp, _db, app) = app_with_tracks().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    let manifest = json!({
        "id": "lrclib-lyrics", "name": "LRCLIB Lyrics", "version": "0.1.0", "apiVersion": "0.2",
        "permissions": [
            { "permission": "libraryRead", "required": true, "reason": "To find tracks." },
            { "permission": "network", "required": true, "reason": "To fetch lyrics.",
              "destinations": ["lrclib.net"] },
            { "permission": "libraryAdd", "required": false, "reason": "To save .lrc files." }
        ]
    });
    conn.execute(
        "INSERT INTO plugins (id, manifest, installed_at, updated_at) VALUES ('lrclib-lyrics', ?1, 0, 0)",
        [manifest.to_string()],
    )
    .unwrap();
    (temp, app)
}

const PLUGIN: &str = "/api/v1/admin/plugins/lrclib-lyrics";

#[tokio::test]
async fn only_admins_administer_plugins() {
    let (temp, app) = server().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    for (method, uri) in [
        ("GET", "/api/v1/admin/plugins"),
        ("POST", "/api/v1/admin/plugins"),
        ("GET", PLUGIN),
        ("DELETE", PLUGIN),
        ("PUT", "/api/v1/admin/plugins/lrclib-lyrics/permissions"),
        ("PUT", "/api/v1/admin/plugins/lrclib-lyrics/libraries/1"),
        ("POST", "/api/v1/admin/plugins/lrclib-lyrics/run"),
    ] {
        let (status, problem) = call(&app, "Bearer user-token", method, uri, Some(json!({}))).await;
        assert_eq!(
            (status, &problem["code"]),
            (StatusCode::FORBIDDEN, &json!("forbidden")),
            "{method} {uri}"
        );
        assert_eq!(
            call(&app, "", method, uri, None).await.0,
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }
    assert_eq!(call(&app, ADMIN, "GET", "/api/v1/admin/plugins", None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_file_that_is_not_a_plugin_is_refused_and_nothing_is_kept() {
    let (temp, app) = server().await;
    let request = Request::post("/api/v1/admin/plugins")
        .header(header::AUTHORIZATION, ADMIN)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from("hello"))
        .unwrap();
    let (status, _, body) = respond(app.clone(), request).await;
    let problem: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::UNPROCESSABLE_ENTITY, &json!("plugin_invalid"))
    );
    assert_eq!(problem["detail"], "This is not a WebAssembly file.");
    assert!(!temp.path().join("plugins").exists());
    let (_, list) = call(&app, ADMIN, "GET", "/api/v1/admin/plugins", None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "only the one installed before");
}

#[tokio::test]
async fn is_installed_disabled_everywhere_with_nothing_granted() {
    let (_temp, app) = server().await;
    let (status, plugin) = call(&app, ADMIN, "GET", PLUGIN, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plugin["source"], json!({ "kind": "file" }));
    assert_eq!(plugin["permissions"].as_array().unwrap().len(), 3);
    assert_eq!(plugin["granted"], json!([]));
    assert_eq!(
        plugin["libraries"][0],
        json!({ "libraryId": "1", "enabled": false, "autoDisabled": false, "disabledReason": null,
                "granted": [], "missingRequired": ["libraryRead", "network"] })
    );
    assert_eq!(plugin["libraries"].as_array().unwrap().len(), 2, "every library");
    assert_eq!(
        call(&app, ADMIN, "GET", "/api/v1/admin/plugins/nothing", None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn cannot_be_enabled_until_its_required_permissions_are_granted() {
    let (_temp, app) = server().await;
    let enable = |library: &str| format!("{PLUGIN}/libraries/{library}");
    let (status, problem) =
        call(&app, ADMIN, "PUT", &enable("1"), Some(json!({ "enabled": true }))).await;
    assert_eq!((status, &problem["code"]), (StatusCode::CONFLICT, &json!("permissions_required")));
    assert_eq!(problem["detail"], "It still needs Read the library and Network access.");

    // Ignored: a permission it never asked for, one in the wrong scope, and an unknown library.
    let grants = json!({
        "granted": ["network", "listeningActivity", "libraryRead"],
        "libraries": [
            { "libraryId": "1", "granted": ["libraryRead", "network"] },
            { "libraryId": "9", "granted": ["libraryRead"] }
        ]
    });
    let (status, plugin) =
        call(&app, ADMIN, "PUT", &format!("{PLUGIN}/permissions"), Some(grants)).await;
    assert_eq!(status, StatusCode::OK, "{plugin}");
    assert_eq!(plugin["granted"], json!(["network"]));
    assert_eq!(plugin["libraries"][0]["granted"], json!(["libraryRead"]));
    assert_eq!(plugin["libraries"][0]["missingRequired"], json!([]));
    assert_eq!(plugin["libraries"][1]["missingRequired"], json!(["libraryRead"]));

    let (status, plugin) =
        call(&app, ADMIN, "PUT", &enable("1"), Some(json!({ "enabled": true }))).await;
    assert_eq!((status, &plugin["libraries"][0]["enabled"]), (StatusCode::OK, &json!(true)));
    assert_eq!(
        call(&app, ADMIN, "PUT", &enable("9"), Some(json!({ "enabled": true }))).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn revoking_a_required_permission_disables_it_at_once() {
    let (_temp, app) = server().await;
    let all = json!({ "granted": ["network"], "libraries": [{ "libraryId": "1", "granted": ["libraryRead"] }] });
    call(&app, ADMIN, "PUT", &format!("{PLUGIN}/permissions"), Some(all)).await;
    call(&app, ADMIN, "PUT", &format!("{PLUGIN}/libraries/1"), Some(json!({ "enabled": true })))
        .await;

    let revoked = json!({ "granted": [], "libraries": [] });
    let (_, plugin) =
        call(&app, ADMIN, "PUT", &format!("{PLUGIN}/permissions"), Some(revoked)).await;
    let library = &plugin["libraries"][0];
    assert_eq!((&library["enabled"], &library["autoDisabled"]), (&json!(false), &json!(true)));
    assert_eq!(library["disabledReason"], "A required permission was revoked.");
    assert_eq!(library["granted"], json!(["libraryRead"]), "only what was replaced changed");
}

#[tokio::test]
async fn uninstalling_keeps_its_grants_for_a_reinstall() {
    let (temp, app) = server().await;
    let grants = json!({ "granted": ["network"], "libraries": [{ "libraryId": "2", "granted": ["libraryAdd"] }] });
    call(&app, ADMIN, "PUT", &format!("{PLUGIN}/permissions"), Some(grants)).await;
    assert_eq!(call(&app, ADMIN, "DELETE", PLUGIN, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, ADMIN, "GET", PLUGIN, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, ADMIN, "DELETE", PLUGIN, None).await.0, StatusCode::NOT_FOUND);
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    let kept: i64 = conn
        .query_row(
            "SELECT (SELECT count(*) FROM plugin_grants) + (SELECT count(*) FROM plugin_library_grants)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, 2);
}

#[tokio::test]
async fn a_run_needs_a_library_it_is_enabled_in() {
    let (_temp, app) = server().await;
    let (status, result) = call(&app, ADMIN, "POST", &format!("{PLUGIN}/run"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        result,
        json!({ "ok": false, "summary": "LRCLIB Lyrics is not enabled in any library.", "log": [],
                "saved": 0, "scannerRunning": false })
    );
    assert_eq!(
        call(&app, ADMIN, "POST", "/api/v1/admin/plugins/nothing/run", None).await.0,
        StatusCode::NOT_FOUND
    );
}

/// The whole path with a real plugin: install the file, grant, enable, and run. It reaches no
/// network: the run is granted reading and writing, but not the network the plugin requires.
#[tokio::test]
#[ignore = "needs plugins/lrclib-lyrics/build.sh"]
async fn installs_and_runs_a_real_plugin() {
    let (temp, _db, app) = app_with_tracks().await;
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/lrclib-lyrics/target/lrclib-lyrics.wasm"
    );
    let request = Request::post("/api/v1/admin/plugins")
        .header(header::AUTHORIZATION, ADMIN)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(std::fs::read(file).unwrap()))
        .unwrap();
    let (status, _, body) = respond(app.clone(), request).await;
    assert_eq!(status, StatusCode::CREATED, "{}", String::from_utf8_lossy(&body));
    assert!(temp.path().join("plugins/lrclib-lyrics.wasm").is_file());
    let request = Request::post("/api/v1/admin/plugins")
        .header(header::AUTHORIZATION, ADMIN)
        .body(Body::from(std::fs::read(file).unwrap()))
        .unwrap();
    assert_eq!(respond(app.clone(), request).await.0, StatusCode::CONFLICT, "installed twice");

    let grants = json!({ "granted": ["network"], "libraries": [{ "libraryId": "1", "granted": ["libraryRead"] }] });
    call(&app, ADMIN, "PUT", &format!("{PLUGIN}/permissions"), Some(grants)).await;
    let (status, _) = call(
        &app,
        ADMIN,
        "PUT",
        &format!("{PLUGIN}/libraries/1"),
        Some(json!({ "enabled": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Revoke the network behind its back, as the database allows but the API would not.
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    conn.execute("DELETE FROM plugin_grants", []).unwrap();
    let (status, result) = call(&app, ADMIN, "POST", &format!("{PLUGIN}/run"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["ok"], false, "{result}");
    assert_eq!(result["summary"], "It still needs Network access.");
}
