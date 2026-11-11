use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use jewelcase_scanner::{Scanner, ScannerThread};
use rusqlite::params;
use serde_json::{Value, json};

use crate::api::AppState;
use crate::api::testing::{
    TOKEN, add_device, add_user, app, call, conn, eventually, json, router_over, state,
};
use crate::db::testing::{User, access, library};
use crate::db::{SqliteStore, libraries};

const SCANS: &str = "/api/v1/admin/libraries/1/scans";

/// The API with library 1, whose one root is `music` in the temporary folder holding one file in
/// `Album`, and its scanner, which runs until its thread is dropped. The owner is logged in with
/// [`TOKEN`], and user 2, who reaches library 1, with `user-token`.
async fn scanned() -> (tempfile::TempDir, Router, ScannerThread) {
    let (temp, db) = crate::db::testing::open();
    let music = temp.path().join("music");
    std::fs::create_dir_all(music.join("Album")).unwrap();
    jewelcase_scanner::testing::write_small_wav(&music.join("Album/01.wav"));
    let conn = conn(&temp);
    User::new(1, "owner").insert(&conn).unwrap();
    add_device(&conn, 1, 1, TOKEN);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    library(&conn, 1, "Music", &music).unwrap();
    access(&conn, 2, 1).unwrap();
    let config = db.read(libraries::all).await.unwrap()[0].config();
    // Started outside the runtime, as `Scanning::start` starts every scanner.
    let store_db = db.clone();
    let (scanner, thread) = tokio::task::spawn_blocking(move || {
        let store = Arc::new(SqliteStore::new(store_db).unwrap());
        Scanner::start(store, config).unwrap()
    })
    .await
    .unwrap();
    let scanners = Arc::new(HashMap::from([(1, scanner)]));
    let app = router_over(AppState { scanners, ..state(&temp, db) });
    (temp, app, thread)
}

/// Stops the scanner `thread` started, off the async threads, as dropping it waits for it.
async fn stop(thread: ScannerThread) {
    tokio::task::spawn_blocking(move || drop(thread)).await.unwrap();
}

#[tokio::test]
async fn an_admin_starts_a_scan_and_sees_it_finish() {
    let (_temp, app, thread) = scanned().await;

    let (status, started) = call(&app, TOKEN, "POST", SCANS, None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(started["trigger"], "manual");
    assert_eq!(started["wholeLibrary"], true, "a manual scan is of every root");
    assert!(["queued", "running"].contains(&started["state"].as_str().unwrap()), "{started}");

    let finished = eventually("the scan finishes", async || {
        let (_, list) = json(&app, SCANS).await;
        let items = list["items"].as_array().unwrap().clone();
        (items.len() == 1 && items[0]["state"] == "completed").then(|| items[0].clone())
    })
    .await;
    assert_eq!(finished["id"], started["id"], "the scan the request queued");
    assert_eq!((&finished["filesSeen"], &finished["added"]), (&json!(1), &json!(1)));
    assert!(finished["startedAt"].is_string() && finished["finishedAt"].is_string());
    assert_eq!(finished["currentFolder"], Value::Null, "a finished scan is in no folder");
    stop(thread).await;
}

/// Scanning is an admin's (`requirements/users.md` §1), and a user who reaches the library is
/// told they may not, not that it is missing (§10).
#[tokio::test]
async fn a_user_may_not_start_or_see_scans() {
    let (_temp, app, thread) = scanned().await;
    for method in ["POST", "GET"] {
        let (status, problem) = call(&app, "user-token", method, SCANS, None).await;
        assert_eq!((status, &problem["code"]), (StatusCode::FORBIDDEN, &json!("forbidden")));
    }
    let (_, list) = json(&app, SCANS).await;
    assert_eq!(list["items"], json!([]), "the refused request queued nothing");
    stop(thread).await;
}

#[tokio::test]
async fn a_library_that_does_not_exist_is_not_found() {
    let (_temp, app, thread) = scanned().await;
    for method in ["POST", "GET"] {
        let (status, _) = call(&app, TOKEN, method, "/api/v1/admin/libraries/9/scans", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
    }
    stop(thread).await;
}

/// What is under way and the last scan to finish, newest first, and nothing of another library.
/// The last to finish is the last that ran, so a scan cancelled since is passed over, and a row
/// that cannot be read hides only itself.
#[tokio::test]
async fn the_list_is_what_is_under_way_and_the_last_finished_scan() {
    let (temp, _db, app) = app("");
    let conn = conn(&temp);
    library(&conn, 1, "Music", "/music").unwrap();
    library(&conn, 2, "Other", "/other").unwrap();
    let root: i64 = conn
        .query_row("SELECT id FROM library_roots WHERE library_id = 1", [], |r| r.get(0))
        .unwrap();
    let insert = "INSERT INTO scans (id, library_id, trigger, state, scopes, created_at, \
                  started_at, finished_at, files_seen, current_root_id, current_path) \
                  VALUES (?1, ?2, ?3, ?4, '[]', ?5, ?6, ?7, ?8, ?9, ?10)";
    // ID, library, trigger, state, created, started, finished, files seen, current folder.
    type Row = (
        i64,
        i64,
        &'static str,
        &'static str,
        i64,
        Option<i64>,
        Option<i64>,
        i64,
        Option<&'static str>,
    );
    let rows: [Row; 6] = [
        (11, 1, "scheduled", "completed", 1, Some(1), Some(2), 10, None),
        (12, 1, "watch", "suspended", 2, Some(2), Some(3), 4, None),
        (13, 1, "watch", "cancelled", 3, None, Some(4), 0, None),
        (14, 1, "scheduled", "running", 4, Some(4), None, 7, Some("Album/Disc 1")),
        (15, 1, "manual", "queued", 5, None, None, 0, None),
        (16, 2, "manual", "queued", 6, None, None, 0, None),
    ];
    for (id, lib, trigger, state, created, started, finished, seen, folder) in rows {
        let root = folder.map(|_| root);
        conn.execute(
            insert,
            params![id, lib, trigger, state, created, started, finished, seen, root, folder],
        )
        .unwrap();
    }
    // A folder that is not UTF-8, which no scan writes, so the row cannot be read.
    conn.execute(
        "INSERT INTO scans (id, library_id, trigger, state, scopes, created_at, current_root_id, \
         current_path) VALUES (17, 1, 'manual', 'queued', '[]', 7, ?1, CAST(x'ff' AS TEXT))",
        [root],
    )
    .unwrap();
    // Scopes that are an array, as the column requires, but of something no scan writes.
    conn.execute(
        "INSERT INTO scans (id, library_id, trigger, state, scopes, created_at) \
         VALUES (18, 1, 'manual', 'queued', '[\"x\", 1, null]', 8)",
        [],
    )
    .unwrap();

    let (status, list) = json(&app, SCANS).await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    let ids: Vec<&Value> = items.iter().map(|scan| &scan["id"]).collect();
    assert_eq!(
        ids,
        [&json!("18"), &json!("15"), &json!("14"), &json!("12")],
        "queued, up next, running, last that ran"
    );
    assert_eq!(items[0]["wholeLibrary"], false, "scopes it cannot read cover nothing");
    assert_eq!(items[2]["currentFolder"], "Album/Disc 1", "relative to its root");
    assert_eq!(items[2]["filesSeen"], 7);
    assert_eq!(items[2]["startedAt"], "1970-01-01T00:00:00.004Z");
    assert_eq!(items[0]["startedAt"], Value::Null, "a queued scan has not started");
}

/// A scan covers the whole library only when it scans every current root from the top down. A
/// retry that covers only the roots that did not answer keeps its trigger, so only this tells it
/// from a scan of the library (`design/scanning.md` §9).
#[tokio::test]
async fn a_scan_says_whether_it_covers_the_whole_library() {
    let (temp, _db, app) = app("");
    let conn = conn(&temp);
    libraries::create(&conn, Some(1), "Music", &["/a".as_ref(), "/b".as_ref()], &[]).unwrap();
    // A root removed since is not one the library still has to cover.
    conn.execute(
        "INSERT INTO library_roots (library_id, path, created_at, removed_at) VALUES (1, '/c', 0, 1)",
        [],
    )
    .unwrap();
    let root = |path: &str| -> i64 {
        conn.query_row("SELECT id FROM library_roots WHERE path = ?1", [path], |r| r.get(0))
            .unwrap()
    };
    let (a, b, c) = (root("/a"), root("/b"), root("/c"));
    let scope =
        |root: i64, path: &str, depth: &str| json!({ "root": root, "path": path, "depth": depth });
    let scans = [
        (21, json!([scope(a, "", "subtree"), scope(b, "", "subtree")]), true),
        (22, json!([scope(b, "", "subtree")]), false),
        (23, json!([scope(a, "", "subtree"), scope(b, "Album", "subtree")]), false),
        (24, json!([scope(a, "", "subtree"), scope(b, "", "directory")]), false),
        (
            25,
            json!([scope(a, "", "subtree"), scope(b, "", "subtree"), scope(c, "", "subtree")]),
            true,
        ),
    ];
    for (id, scopes, _) in &scans {
        conn.execute(
            "INSERT INTO scans (id, library_id, trigger, state, scopes, created_at) \
             VALUES (?1, 1, 'scheduled', 'queued', ?2, ?1)",
            params![id, scopes.to_string()],
        )
        .unwrap();
    }

    let (_, list) = json(&app, SCANS).await;
    let whole: HashMap<String, bool> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|scan| {
            (scan["id"].as_str().unwrap().to_owned(), scan["wholeLibrary"].as_bool().unwrap())
        })
        .collect();
    for (id, _, expected) in scans {
        assert_eq!(whole[&id.to_string()], expected, "scan {id}");
    }
}
