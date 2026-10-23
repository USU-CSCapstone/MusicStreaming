use std::io;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode, header};
use futures_util::stream;
use serde_json::{Value, json};

use super::super::testing::{app_with_tracks, respond};
use crate::db::Database;
use crate::plugins::settings;
use crate::plugins::testing::{enable_ungranted, install, manifest};

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
async fn server() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app_with_tracks().await;
    let mut lyrics = manifest("lrclib-lyrics", &[]);
    lyrics["name"] = json!("LRCLIB Lyrics");
    lyrics["permissions"] = json!([
        { "permission": "libraryRead", "required": true, "reason": "To find tracks." },
        { "permission": "network", "required": true, "reason": "To fetch lyrics.",
          "destinations": ["lrclib.net"] },
        { "permission": "libraryAdd", "required": false, "reason": "To save .lrc files." }
    ]);
    lyrics["settings"] = json!({
        "properties": {
            "apiKey": { "type": "string", "title": "API key", "writeOnly": true },
            "mode": { "type": "string", "enum": ["fast", "careful"], "default": "fast" },
            "limit": { "type": "integer", "title": "Limit" }
        },
        "required": ["apiKey"]
    });
    install(&db, lyrics).await;
    (temp, db, app)
}

const PLUGIN: &str = "/api/v1/admin/plugins/lrclib-lyrics";

#[tokio::test]
async fn a_file_that_is_not_a_plugin_is_refused_and_nothing_is_kept() {
    let (temp, _db, app) = server().await;
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

/// Uploads `chunks` as a plugin file, and returns the status and Problem.
async fn upload(app: &Router, chunks: Vec<io::Result<Bytes>>) -> (StatusCode, Value) {
    let request = Request::post("/api/v1/admin/plugins")
        .header(header::AUTHORIZATION, ADMIN)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from_stream(stream::iter(chunks)))
        .unwrap();
    let (status, _, body) = respond(app.clone(), request).await;
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn a_file_over_the_limit_is_refused_as_too_big() {
    let (_temp, _db, app) = server().await;
    let megabyte = Bytes::from(vec![0; 1 << 20]);
    let chunks = (0..=jewelcase_plugins::MAX_SIZE >> 20).map(|_| Ok(megabyte.clone())).collect();
    let (status, problem) = upload(&app, chunks).await;
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::UNPROCESSABLE_ENTITY, &json!("plugin_invalid"))
    );
    assert_eq!(problem["detail"], "The file is over 50 MB.");
}

#[tokio::test]
async fn a_file_over_the_default_body_limit_but_under_the_plugin_limit_is_read() {
    let (_temp, _db, app) = server().await;
    // Three times the 2 MB a request body may otherwise be.
    let megabyte = Bytes::from(vec![0; 1 << 20]);
    let (status, problem) = upload(&app, (0..6).map(|_| Ok(megabyte.clone())).collect()).await;
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::UNPROCESSABLE_ENTITY, &json!("plugin_invalid"))
    );
    assert_eq!(problem["detail"], "This is not a WebAssembly file.");
}

#[tokio::test]
async fn an_upload_that_breaks_off_is_not_called_too_big() {
    let (_temp, _db, app) = server().await;
    // What the server's body timeout makes of an upload that stalls.
    let stalled = io::Error::new(io::ErrorKind::TimedOut, "stalled");
    let (status, problem) = upload(&app, vec![Ok(Bytes::from("\0asm")), Err(stalled)]).await;
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::UNPROCESSABLE_ENTITY, &json!("validation_failed"))
    );
    assert_eq!(problem["detail"], "The upload did not arrive in full.");
}

#[tokio::test]
async fn is_installed_disabled_everywhere_with_nothing_granted() {
    let (_temp, _db, app) = server().await;
    let (status, plugin) = call(&app, ADMIN, "GET", PLUGIN, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plugin["source"], json!({ "kind": "file" }));
    assert_eq!(plugin["permissions"].as_array().unwrap().len(), 3);
    assert_eq!(plugin["granted"], json!([]));
    assert_eq!(
        plugin["libraries"][0],
        json!({ "libraryId": "1", "enabled": false, "autoDisabled": false, "disabledReason": null,
                "granted": [], "missingRequired": ["libraryRead", "network"], "lastRun": null })
    );
    assert_eq!(plugin["libraries"].as_array().unwrap().len(), 2, "every library");
    assert_eq!(
        call(&app, ADMIN, "GET", "/api/v1/admin/plugins/nothing", None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn its_times_are_rfc_3339_with_milliseconds() {
    let (_temp, db, app) = server().await;
    db.write(|tx| {
        tx.execute("UPDATE plugins SET installed_at = 951782400123", [])?;
        tx.execute(
            "INSERT INTO plugin_libraries (plugin_id, library_id, enabled, last_run_at, \
             last_run_ok, last_run_summary) VALUES ('lrclib-lyrics', 1, 0, 4102444799999, 1, 'Ran.')",
            [],
        )
    })
    .await
    .unwrap();
    let (_, plugin) = call(&app, ADMIN, "GET", PLUGIN, None).await;
    assert_eq!(plugin["installedAt"], "2000-02-29T00:00:00.123Z");
    assert_eq!(
        plugin["libraries"][0]["lastRun"],
        json!({ "at": "2099-12-31T23:59:59.999Z", "ok": true, "summary": "Ran." })
    );
}

/// Granting, enabling, and settings answer `404` for a plugin that is not installed.
#[tokio::test]
async fn a_plugin_not_installed_is_not_found() {
    let (_temp, _db, app) = server().await;
    let nothing = "/api/v1/admin/plugins/nothing";
    let grants = json!({ "granted": [], "libraries": [] });
    for (method, path, body) in [
        ("PUT", format!("{nothing}/permissions"), Some(grants)),
        ("PUT", format!("{nothing}/libraries/1"), Some(json!({ "enabled": false }))),
        ("PUT", format!("{nothing}/settings"), Some(json!({ "values": {} }))),
        ("GET", format!("{nothing}/settings"), None),
    ] {
        let (status, problem) = call(&app, ADMIN, method, &path, body).await;
        assert_eq!(
            (status, &problem["code"]),
            (StatusCode::NOT_FOUND, &json!("not_found")),
            "{path}"
        );
    }
}

#[tokio::test]
async fn cannot_be_enabled_until_its_required_permissions_are_granted() {
    let (_temp, _db, app) = server().await;
    let enable = |library: &str| format!("{PLUGIN}/libraries/{library}");
    let (status, problem) =
        call(&app, ADMIN, "PUT", &enable("1"), Some(json!({ "enabled": true }))).await;
    assert_eq!((status, &problem["code"]), (StatusCode::CONFLICT, &json!("permissions_required")));
    assert_eq!(problem["detail"], "It still needs Read the library and Network access.");

    // Ignored: a permission it never asked for, one in the wrong scope, and an unknown or
    // malformed library.
    let grants = json!({
        "granted": ["network", "listeningActivity", "libraryRead"],
        "libraries": [
            { "libraryId": "1", "granted": ["libraryRead", "network"] },
            { "libraryId": "9", "granted": ["libraryRead"] },
            { "libraryId": "02", "granted": ["libraryAdd"] }
        ]
    });
    let (status, plugin) =
        call(&app, ADMIN, "PUT", &format!("{PLUGIN}/permissions"), Some(grants)).await;
    assert_eq!(status, StatusCode::OK, "{plugin}");
    assert_eq!(plugin["granted"], json!(["network"]));
    assert_eq!(plugin["libraries"][0]["granted"], json!(["libraryRead"]));
    assert_eq!(plugin["libraries"][0]["missingRequired"], json!([]));
    assert_eq!(plugin["libraries"][1]["missingRequired"], json!(["libraryRead"]));
    assert_eq!(
        (&plugin["libraries"][1]["libraryId"], &plugin["libraries"][1]["granted"]),
        (&json!("2"), &json!([])),
        "02 is not library 2"
    );

    // Granted in library 2 as well, it is enabled in the library the path names and no other.
    let grants = json!({
        "granted": ["network"],
        "libraries": [
            { "libraryId": "1", "granted": ["libraryRead"] },
            { "libraryId": "2", "granted": ["libraryRead"] }
        ]
    });
    let (status, plugin) =
        call(&app, ADMIN, "PUT", &format!("{PLUGIN}/permissions"), Some(grants)).await;
    assert_eq!(status, StatusCode::OK, "{plugin}");
    let (status, plugin) =
        call(&app, ADMIN, "PUT", &enable("2"), Some(json!({ "enabled": true }))).await;
    let enabled = |plugin: &Value| {
        plugin["libraries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|library| library["enabled"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!((status, enabled(&plugin)), (StatusCode::OK, vec![json!(false), json!(true)]));
    let (status, plugin) =
        call(&app, ADMIN, "PUT", &enable("1"), Some(json!({ "enabled": true }))).await;
    assert_eq!((status, enabled(&plugin)), (StatusCode::OK, vec![json!(true), json!(true)]));
    assert_eq!(
        call(&app, ADMIN, "PUT", &enable("9"), Some(json!({ "enabled": true }))).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_limit_for_every_plugin_is_granted_like_any_other_permission_and_shown_where_it_binds() {
    let (_temp, db, app) = server().await;
    let mut artwork = manifest("artwork", &[]);
    artwork["permissions"] = json!([
        { "permission": "network", "required": true, "reason": "To fetch artwork.",
          "destinations": ["musicbrainz.org"],
          "rateLimits": { "musicbrainz.org": { "limit": "1/s", "per": "host" } } },
        { "permission": "hostRateLimits", "required": false,
          "reason": "MusicBrainz limits each server, not each plugin." }
    ]);
    install(&db, artwork).await;
    let mut anywhere = manifest("anywhere", &[]);
    anywhere["permissions"] = json!([{ "permission": "network", "required": true,
                                       "reason": "To look.", "destinations": ["*"] }]);
    install(&db, anywhere).await;
    let artwork = "/api/v1/admin/plugins/artwork";
    let grants = json!({ "granted": ["network", "hostRateLimits"], "libraries": [] });
    let (status, plugin) =
        call(&app, ADMIN, "PUT", &format!("{artwork}/permissions"), Some(grants)).await;
    assert_eq!(status, StatusCode::OK, "{plugin}");
    assert_eq!(plugin["granted"], json!(["hostRateLimits", "network"]));
    // The admin sees the limit before granting it.
    let limits = &plugin["permissions"][0]["rateLimits"];
    assert_eq!(limits, &json!({ "musicbrainz.org": { "limit": "1/s", "per": "host" } }));
    assert_eq!(plugin["hostLimits"], json!([]), "not in force until it is enabled");

    let enable = Some(json!({ "enabled": true }));
    call(&app, ADMIN, "PUT", &format!("{artwork}/libraries/1"), enable).await;
    let in_force =
        json!({ "host": "musicbrainz.org", "limit": "1/s", "setBy": ["artwork"], "exempt": false });
    let (_, plugin) = call(&app, ADMIN, "GET", "/api/v1/admin/plugins/anywhere", None).await;
    assert_eq!(plugin["hostLimits"], json!([in_force]), "it reaches any host");
    let (_, plugin) = call(&app, ADMIN, "GET", PLUGIN, None).await;
    assert_eq!(plugin["hostLimits"], json!([]), "it reaches only lrclib.net");
}

#[tokio::test]
async fn an_admin_exempts_a_plugin_from_limits_for_every_plugin_one_host_at_a_time() {
    let (_temp, db, app) = server().await;
    let mut plugin = manifest("setter", &[]);
    plugin["permissions"] = json!([
        { "permission": "network", "required": true, "reason": "To look.",
          "destinations": ["lrclib.net"],
          "rateLimits": { "lrclib.net": { "limit": "1/s", "per": "host" } } },
        { "permission": "hostRateLimits", "required": true, "reason": "To share." }
    ]);
    install(&db, plugin).await;
    let setter = "/api/v1/admin/plugins/setter";
    let grants = Some(json!({ "granted": ["network", "hostRateLimits"], "libraries": [] }));
    call(&app, ADMIN, "PUT", &format!("{setter}/permissions"), grants).await;
    call(&app, ADMIN, "PUT", &format!("{setter}/libraries/1"), Some(json!({ "enabled": true })))
        .await;
    let exempt = || async {
        call(&app, ADMIN, "GET", PLUGIN, None).await.1["hostLimits"][0]["exempt"].clone()
    };
    assert_eq!(exempt().await, json!(false));

    // Each is idempotent, so a repeat changes nothing.
    let lrclib = format!("{PLUGIN}/exemptions/LRCLIB.net");
    for (method, now) in [("PUT", true), ("PUT", true), ("DELETE", false), ("DELETE", false)] {
        let (status, body) = call(&app, ADMIN, method, &lrclib, None).await;
        assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null), "{method}");
        assert_eq!(exempt().await, json!(now), "{method}");
    }

    for wrong in ["*", "*.last.fm", "last.fm:443", "a%2Fb"] {
        for method in ["PUT", "DELETE"] {
            let uri = format!("{PLUGIN}/exemptions/{wrong}");
            let (status, problem) = call(&app, ADMIN, method, &uri, None).await;
            assert_eq!(
                (status, &problem["code"]),
                (StatusCode::UNPROCESSABLE_ENTITY, &json!("validation_failed")),
                "{method} {wrong}"
            );
        }
    }
    let elsewhere = "/api/v1/admin/plugins/nothing/exemptions/lrclib.net";
    for method in ["PUT", "DELETE"] {
        assert_eq!(call(&app, ADMIN, method, elsewhere, None).await.0, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn revoking_a_required_permission_disables_it_at_once() {
    let (_temp, _db, app) = server().await;
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
async fn naming_library_0_leaves_the_plugins_own_grants() {
    let (_temp, _db, app) = server().await;
    let path = format!("{PLUGIN}/permissions");
    let all = json!({ "granted": ["network"], "libraries": [{ "libraryId": "1", "granted": ["libraryRead"] }] });
    call(&app, ADMIN, "PUT", &path, Some(all)).await;
    let zero = json!({ "granted": ["network"], "libraries": [{ "libraryId": "0", "granted": ["libraryRead"] }] });
    let (status, plugin) = call(&app, ADMIN, "PUT", &path, Some(zero)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plugin["granted"], json!(["network"]));
    assert_eq!(plugin["libraries"][0]["granted"], json!(["libraryRead"]));
}

#[tokio::test]
async fn uninstalling_keeps_its_grants_for_a_reinstall() {
    let (temp, _db, app) = server().await;
    let grants = json!({ "granted": ["network"], "libraries": [{ "libraryId": "2", "granted": ["libraryAdd"] }] });
    call(&app, ADMIN, "PUT", &format!("{PLUGIN}/permissions"), Some(grants)).await;
    assert_eq!(call(&app, ADMIN, "DELETE", PLUGIN, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, ADMIN, "GET", PLUGIN, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, ADMIN, "DELETE", PLUGIN, None).await.0, StatusCode::NOT_FOUND);
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    let kept: i64 =
        conn.query_row("SELECT count(*) FROM plugin_grants", [], |row| row.get(0)).unwrap();
    assert_eq!(kept, 2);
}

#[tokio::test]
async fn a_run_needs_a_library_it_is_enabled_in() {
    let (_temp, _db, app) = server().await;
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

/// Installs the real lyrics plugin through the API, and returns its file's bytes.
async fn install_lyrics(app: &Router) -> Vec<u8> {
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/lrclib-lyrics/target/lrclib-lyrics.wasm"
    );
    let bytes = std::fs::read(file).expect("run plugins/lrclib-lyrics/build.sh first");
    let request = Request::post("/api/v1/admin/plugins")
        .header(header::AUTHORIZATION, ADMIN)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(bytes.clone()))
        .unwrap();
    let (status, _, body) = respond(app.clone(), request).await;
    assert_eq!(status, StatusCode::CREATED, "{}", String::from_utf8_lossy(&body));
    bytes
}

/// Saves server-wide settings for the lyrics plugin, which runs it to check them, and returns
/// the status and body.
async fn check_settings(app: &Router) -> (StatusCode, Value) {
    let values = Some(json!({ "values": { "syncedOnly": true } }));
    call(app, ADMIN, "PUT", "/api/v1/admin/plugins/lrclib-lyrics/settings", values).await
}

/// The whole path with a real plugin: install the file, grant, enable, and run. It reaches no
/// network: it is granted reading, and enabled without the network it requires, as only the
/// database allows.
#[tokio::test]
async fn installs_and_runs_a_real_plugin() {
    let (temp, db, app) = app_with_tracks().await;
    let bytes = install_lyrics(&app).await;
    assert!(temp.path().join("plugins/lrclib-lyrics.wasm").is_file());
    assert!(temp.path().join("compiled/modules").is_dir(), "kept compiled where it was told");
    let request = Request::post("/api/v1/admin/plugins")
        .header(header::AUTHORIZATION, ADMIN)
        .body(Body::from(bytes))
        .unwrap();
    assert_eq!(respond(app.clone(), request).await.0, StatusCode::CONFLICT, "installed twice");
    let parts = std::fs::read_dir(temp.path().join("plugins"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap());
    let parts: Vec<_> = parts.filter(|name| name.ends_with(".part")).collect();
    assert!(parts.is_empty(), "the refused upload is not kept: {parts:?}");

    let grants =
        json!({ "granted": [], "libraries": [{ "libraryId": "1", "granted": ["libraryRead"] }] });
    let (status, _) =
        call(&app, ADMIN, "PUT", &format!("{PLUGIN}/permissions"), Some(grants)).await;
    assert_eq!(status, StatusCode::OK);
    enable_ungranted(&db, "lrclib-lyrics").await;
    let (status, result) = call(&app, ADMIN, "POST", &format!("{PLUGIN}/run"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["ok"], false, "{result}");
    assert_eq!(result["summary"], "It still needs Network access.");
    let (_, plugin) = call(&app, ADMIN, "GET", PLUGIN, None).await;
    assert_eq!(plugin["libraries"][0]["lastRun"], Value::Null, "a refused run is not recorded");

    // Settings pass its own check, which it answers by running, and are kept.
    let (status, saved) = check_settings(&app).await;
    assert_eq!(
        (status, &saved["values"]),
        (StatusCode::OK, &json!({ "syncedOnly": true })),
        "{saved}"
    );
}

/// The component compiled to check the file at install is the one its runs use, so a run does
/// not read the file again.
#[tokio::test]
async fn an_installed_plugin_runs_as_compiled_at_install() {
    let (temp, _db, app) = app_with_tracks().await;
    install_lyrics(&app).await;
    std::fs::remove_file(temp.path().join("plugins/lrclib-lyrics.wasm")).unwrap();
    let (status, saved) = check_settings(&app).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
}

/// Uninstalling forgets the compiled plugin: one recorded as installed again without its file
/// cannot be loaded.
#[tokio::test]
async fn an_uninstalled_plugin_is_not_kept_compiled() {
    let (_temp, db, app) = app_with_tracks().await;
    let bytes = install_lyrics(&app).await;
    assert_eq!(call(&app, ADMIN, "DELETE", PLUGIN, None).await.0, StatusCode::NO_CONTENT);
    let manifest = jewelcase_plugins::manifest::read(&bytes).unwrap();
    install(&db, serde_json::to_value(manifest).unwrap()).await;
    let (status, problem) = check_settings(&app).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    let detail = problem["detail"].as_str().unwrap();
    assert!(detail.starts_with("the plugin could not be loaded"), "{detail}");
}

const SETTINGS: &str = "/api/v1/admin/plugins/lrclib-lyrics/settings";

/// Server-wide settings that cannot be read fail their GET, as they would every run, until an
/// admin saves settings again: the save writes over them, and they then read as saved.
#[tokio::test]
async fn a_save_mends_server_settings_that_cannot_be_read() {
    let (_temp, db, app) = app_with_tracks().await;
    install_lyrics(&app).await;
    // SQLite takes a number too large for serde as JSON.
    let unreadable = r#"{"syncedOnly": true, "limit": 1e400}"#;
    db.write(move |tx| {
        let sql = "INSERT INTO plugin_settings VALUES ('lrclib-lyrics', 0, ?1)";
        tx.execute(sql, [unreadable])
    })
    .await
    .unwrap();
    let (status, _) = call(&app, ADMIN, "GET", SETTINGS, None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    let (status, saved) = check_settings(&app).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, settings) = call(&app, ADMIN, "GET", SETTINGS, None).await;
    assert_eq!((status, &settings["values"]), (StatusCode::OK, &json!({ "syncedOnly": true })));
    // What a run in a library reads.
    let effective = db.read(|conn| settings::effective(conn, "lrclib-lyrics", None, 1)).await;
    assert_eq!(Value::Object(effective.unwrap()), json!({ "syncedOnly": true }));
}

#[tokio::test]
async fn settings_show_values_but_never_secrets() {
    let (temp, _db, app) = server().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    conn.execute(
        r#"INSERT INTO plugin_settings VALUES ('lrclib-lyrics', 0, '{"apiKey":"s3cret","limit":4}')"#,
        [],
    )
    .unwrap();
    let (status, settings) = call(&app, ADMIN, "GET", SETTINGS, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["values"], json!({ "limit": 4 }));
    assert_eq!(settings["secretsSet"], json!(["apiKey"]));
    assert_eq!(settings["schema"]["type"], "object");
    assert_eq!(settings["schema"]["properties"]["apiKey"]["writeOnly"], true);
    assert!(!settings.to_string().contains("s3cret"));

    let (status, library) =
        call(&app, ADMIN, "GET", &format!("{SETTINGS}?libraryId=2"), None).await;
    assert_eq!(
        (status, &library["values"]),
        (StatusCode::OK, &json!({})),
        "a library sets nothing of its own"
    );
    for library in ["9", "x"] {
        let uri = format!("{SETTINGS}?libraryId={library}");
        assert_eq!(
            call(&app, ADMIN, "GET", &uri, None).await.0,
            StatusCode::NOT_FOUND,
            "{library}"
        );
    }
    let (_, plugin) = call(&app, ADMIN, "GET", PLUGIN, None).await;
    assert_eq!(plugin["hasSettings"], true);
}

#[tokio::test]
async fn settings_are_checked_before_the_plugin_sees_them() {
    let (_temp, _db, app) = server().await;
    for (body, detail) in [
        (
            json!({ "limit": 2.5, "mode": "slow" }),
            r#"Limit must be a whole number; mode must be one of "fast", "careful"."#,
        ),
        (json!({ "colour": "red" }), "colour is not one of this plugin's settings."),
    ] {
        let (status, problem) =
            call(&app, ADMIN, "PUT", SETTINGS, Some(json!({ "values": body }))).await;
        assert_eq!(
            (status, &problem["code"]),
            (StatusCode::UNPROCESSABLE_ENTITY, &json!("plugin_settings_invalid"))
        );
        assert_eq!(problem["detail"], detail);
    }
    // A library's settings, with the server-wide ones under them, leave no required one unset.
    let (status, problem) = call(
        &app,
        ADMIN,
        "PUT",
        &format!("{SETTINGS}?libraryId=1"),
        Some(json!({ "values": { "limit": 2 } })),
    )
    .await;
    assert_eq!(
        (status, &problem["detail"]),
        (StatusCode::UNPROCESSABLE_ENTITY, &json!("API key is required."))
    );
}
