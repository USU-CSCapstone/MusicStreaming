use std::io::Read;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use flate2::read::GzDecoder;
use jewelcase_catalog::SCHEMA_VERSION;
use jewelcase_catalog::sync::{Kind, Row, Rows, apply, delete};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use super::*;
use crate::api::testing::catalog;
use crate::api::testing::{
    add_device, add_user, call, conn, eventually, json, owner_request, respond,
};
use crate::db::feed::{self, Entity, Op};
use crate::db::testing::access;

const URI: &str = "/api/v1/libraries/1/catalog";

/// `GET`s the catalog as the owner, asking for gzip if `gzip`.
async fn fetch(app: &Router, uri: &str, gzip: bool) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut request = owner_request("GET", uri);
    if gzip {
        request = request.header(header::ACCEPT_ENCODING, "gzip");
    }
    respond(app.clone(), request.body(Body::empty()).unwrap()).await
}

/// The catalog once its first build is done.
async fn built(app: &Router) -> (HeaderMap, Vec<u8>) {
    eventually("the first catalog build", async || {
        let (status, headers, body) = fetch(app, URI, false).await;
        (status == StatusCode::OK).then_some((headers, body))
    })
    .await
}

/// Opens database bytes as a connection to a copy of them.
fn open(bytes: &[u8]) -> (tempfile::TempDir, Connection) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("device.db");
    std::fs::write(&path, bytes).unwrap();
    let conn = Connection::open(&path).unwrap();
    (temp, conn)
}

/// The feed position in a response's `X-Jewelcase-Seq`.
fn seq(headers: &HeaderMap) -> i64 {
    headers["x-jewelcase-seq"].to_str().unwrap().parse().unwrap()
}

/// Every table's columns, as `table.column`.
fn columns(conn: &Connection) -> Vec<String> {
    conn.prepare(
        "SELECT m.name || '.' || p.name FROM sqlite_schema AS m, pragma_table_info(m.name) AS p \
         WHERE m.type = 'table' ORDER BY 1",
    )
    .unwrap()
    .query_map([], |r| r.get(0))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

async fn record(db: &Database, library: i64, changes: &[(Entity, i64, Op)]) {
    let changes = changes.to_vec();
    db.write(move |tx| {
        for (entity, id, op) in changes {
            feed::record(tx, library, entity, id, op, 0)?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

/// Moves `library`'s feed past a tenth of the test catalog's entities.
async fn stale(db: &Database, library: i64) {
    let ids =
        [(Entity::Track, 1001), (Entity::Track, 1002), (Entity::Artist, 10), (Entity::Artist, 11)];
    record(db, library, &ids.map(|(entity, id)| (entity, id, Op::Upsert))).await;
    record(db, library, &[(Entity::Album, 101, Op::Upsert), (Entity::Album, 102, Op::Upsert)])
        .await;
}

#[tokio::test]
async fn the_first_request_answers_building_and_a_later_one_the_file() {
    let (_temp, db, app) = catalog::app().await;
    record(&db, 1, &[(Entity::Track, 1001, Op::Upsert)]).await;
    record(&db, 2, &[(Entity::Track, 2001, Op::Upsert)]).await;
    let (status, headers, body) = fetch(&app, URI, false).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(headers[header::RETRY_AFTER], "30");
    let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["code"], "catalog_building");

    let (headers, _) = built(&app).await;
    let head: i64 = db
        .read(|c| {
            c.query_row("SELECT max(seq) FROM library_changes WHERE library_id = 1", [], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap();
    assert_eq!(seq(&headers), head, "the library's head, not the server's");
    assert_eq!(headers[header::CONTENT_TYPE], "application/vnd.sqlite3");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store", "no copy left in the HTTP cache");
    let schema = jewelcase_catalog::SCHEMA_VERSION.to_string();
    assert_eq!(headers["x-jewelcase-schema"], schema.as_str());
}

#[tokio::test]
async fn the_snapshot_holds_exactly_the_device_rows_of_its_library_at_its_seq() {
    let (_temp, db, app) = catalog::app().await;
    let (_, bytes) = built(&app).await;
    let (_file, snapshot) = open(&bytes);

    let device = Connection::open_in_memory().unwrap();
    jewelcase_catalog::create_schema(&device).unwrap();
    let mut tables: Vec<(String, String)> = Vec::new();
    for name in columns(&device) {
        let (table, column) = name.split_once('.').unwrap();
        match tables.last_mut() {
            Some((t, cols)) if t == table => cols.push_str(&format!(", {column}")),
            _ => tables.push((table.to_owned(), column.to_owned())),
        }
    }
    assert_eq!(tables.len(), 16);
    let read = |conn: &Connection, table: &str, cols: &str| -> Vec<Vec<rusqlite::types::Value>> {
        let scope = match table {
            "tag_types" => "key IN (SELECT type FROM tags WHERE library_id = 1)",
            "libraries" => "id = 1",
            _ => "library_id = 1",
        };
        let mut stmt = conn
            .prepare(&format!("SELECT {cols} FROM {table} WHERE {scope} ORDER BY 1, 2"))
            .unwrap();
        let width = stmt.column_count();
        stmt.query_map([], |r| (0..width).map(|i| r.get(i)).collect())
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    for (table, cols) in tables {
        let expected = db
            .read({
                let (table, cols) = (table.clone(), cols.clone());
                move |c| Ok(read(c, &table, &cols))
            })
            .await
            .unwrap();
        assert_eq!(read(&snapshot, &table, &cols), expected, "{table}");
    }
    let tracks: i64 = snapshot.query_row("SELECT count(*) FROM tracks", [], |r| r.get(0)).unwrap();
    assert_eq!(tracks, 6, "the other library's track is not in it");
    let version: i32 = snapshot.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(version, jewelcase_catalog::SCHEMA_VERSION);
    let journal: String = snapshot.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(journal, "delete", "the file is a plain rollback-journal database");
}

#[tokio::test]
async fn no_server_only_column_and_no_other_librarys_row_is_in_the_file() {
    let (_temp, db, app) = catalog::app().await;
    let (headers, bytes) = built(&app).await;
    let first = seq(&headers);
    let (_file, snapshot) = open(&bytes);

    let device = Connection::open_in_memory().unwrap();
    jewelcase_catalog::create_schema(&device).unwrap();
    assert_eq!(columns(&snapshot), columns(&device), "exactly the device schema's columns");

    let tables: Vec<String> = snapshot
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for table in tables.iter().filter(|t| *t != "tag_types") {
        let owner = if table == "libraries" { "id" } else { "library_id" };
        let others: i64 = snapshot
            .query_row(&format!("SELECT count(*) FROM {table} WHERE {owner} <> 1"), [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(others, 0, "{table}");
    }
    // Library 2's tags use a type nothing in library 1 does, which the file must not name.
    db.write(|tx| {
        tx.execute("INSERT INTO tag_types (key, name, plugin_id, field, since) VALUES ('secret', 'Secret', 'p', 'F', 1)", [])?;
        tx.execute("INSERT INTO tags (id, library_id, type, name, name_key, sort_key) VALUES (900, 2, 'secret', 'S', 's', x'01')", [])?;
        Ok(())
    })
    .await
    .unwrap();
    stale(&db, 1).await;
    let (_, rebuilt) = eventually("the rebuilt file", async || {
        let (_, headers, body) = fetch(&app, URI, false).await;
        (seq(&headers) > first).then_some((headers, body))
    })
    .await;
    let (_file, snapshot) = open(&rebuilt);
    let types: i64 = snapshot
        .query_row("SELECT count(*) FROM tag_types WHERE key = 'secret'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(types, 0);
}

#[tokio::test]
async fn another_librarys_snapshot_is_not_found() {
    let (temp, _db, app) = catalog::app().await;
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    access(&conn, 2, 2).unwrap();
    built(&app).await;

    let theirs = call(&app, "user-token", "GET", URI, None).await;
    let missing = call(&app, "user-token", "GET", "/api/v1/libraries/9/catalog", None).await;
    assert_eq!(theirs.0, StatusCode::NOT_FOUND);
    assert_eq!(theirs, missing);
    assert!(temp.path().join("cache/catalog/1").is_dir());
    assert!(!temp.path().join("cache/catalog/2").exists(), "a refused request builds nothing");
}

#[tokio::test]
async fn a_stale_snapshot_is_served_while_its_replacement_builds() {
    let (temp, db, app) = catalog::app().await;
    let (first, _) = built(&app).await;
    stale(&db, 1).await;

    let (status, headers, _) = fetch(&app, URI, false).await;
    assert_eq!(status, StatusCode::OK, "the old file answers, not a 503");
    assert_eq!(seq(&headers), seq(&first));

    eventually("the replacement", async || {
        let (_, headers, body) = fetch(&app, URI, false).await;
        (seq(&headers) > seq(&first)).then_some((headers, body))
    })
    .await;
    // The new file is in place a moment before the build deletes the superseded pair.
    let files = eventually("the superseded pair deleted", async || {
        let mut files: Vec<_> = std::fs::read_dir(temp.path().join("cache/catalog/1"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        (files.len() == 2).then_some(files)
    })
    .await;
    assert!(files[0].ends_with(".db") && files[1] == format!("{}.gz", files[0]));
}

#[tokio::test]
async fn the_feed_after_the_snapshots_seq_brings_a_device_current() {
    let (_temp, db, app) = catalog::app().await;
    let (headers, bytes) = built(&app).await;
    let (_file, device) = open(&bytes);

    db.write(|tx| {
        tx.execute("UPDATE tracks SET title = 'Renamed' WHERE id = 1001", [])?;
        tx.execute("DELETE FROM tracks WHERE id = 1002", [])?;
        Ok(())
    })
    .await
    .unwrap();
    record(&db, 1, &[(Entity::Track, 1001, Op::Upsert), (Entity::Track, 1002, Op::Delete)]).await;

    let after = seq(&headers);
    let (_, page) = json(&app, &format!("/api/v1/libraries/1/changes?after={after}")).await;
    for change in page["changes"].as_array().unwrap() {
        let id = change["id"].as_str().unwrap();
        match change["op"].as_str().unwrap() {
            "delete" => delete(&device, Kind::Track, id.parse().unwrap(), 0).unwrap(),
            _ => {
                let tables = change["rows"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(t, rows)| {
                        let rows = rows.as_array().unwrap().iter();
                        (
                            t.clone(),
                            rows.map(|r| r.as_object().unwrap().clone()).collect::<Vec<Row>>(),
                        )
                    })
                    .collect();
                apply(&device, &Rows { kind: Kind::Track, id: id.to_owned(), key: 0, tables })
                    .unwrap();
            }
        }
    }
    let titles = |conn: &Connection| -> Vec<(i64, String)> {
        conn.prepare("SELECT id, title FROM tracks WHERE library_id = 1 ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let server = db.read(move |c| Ok(titles(c))).await.unwrap();
    assert_eq!(titles(&device), server);
    assert!(server.contains(&(1001, "Renamed".to_owned())));
}

#[tokio::test]
async fn the_etag_is_the_sha256_of_the_served_file() {
    let (_temp, _db, app) = catalog::app().await;
    built(&app).await;
    let expected = |bytes: &[u8]| format!("\"{:x}\"", Sha256::digest(bytes));

    let (_, headers, plain) = fetch(&app, URI, false).await;
    assert!(headers.get(header::CONTENT_ENCODING).is_none());
    assert_eq!(headers[header::ETAG], expected(&plain).as_str());

    let (_, zipped_headers, zipped) = fetch(&app, URI, true).await;
    assert_eq!(zipped_headers[header::CONTENT_ENCODING], "gzip");
    let mut inflated = Vec::new();
    GzDecoder::new(&zipped[..]).read_to_end(&mut inflated).unwrap();
    assert_eq!(inflated, plain);
    assert_eq!(zipped_headers[header::ETAG], headers[header::ETAG]);
}

#[tokio::test]
async fn a_snapshot_failing_quick_check_is_not_served() {
    let (temp, _db, _app) = catalog::app().await;
    let dir = temp.path().join("out");
    let failing = |_: &Connection| Err(rusqlite::Error::InvalidQuery);
    let built = build(&temp.path().join("jewelcase.db"), &dir, 1, failing);
    assert!(built.is_err());
    assert!(newest(&dir).unwrap().is_none());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "no leftovers either");
}

#[test]
fn quick_check_reports_a_damaged_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("bad.db");
    let conn = Connection::open(&path).unwrap();
    jewelcase_catalog::create_schema(&conn).unwrap();
    quick_check(&conn).unwrap();
    drop(conn);
    let mut bytes = std::fs::read(&path).unwrap();
    // Page 2 on is table and index content; overwrite one with noise.
    bytes[4096 * 2..4096 * 3].fill(0xAB);
    std::fs::write(&path, bytes).unwrap();
    assert!(quick_check(&Connection::open(&path).unwrap()).is_err());
}

#[tokio::test]
async fn the_sweep_deletes_a_deleted_librarys_files_and_unfinished_builds() {
    let (temp, db, app) = catalog::app().await;
    built(&app).await;
    let cache = temp.path().join("cache/catalog");
    std::fs::create_dir_all(cache.join("2/building-x")).unwrap();
    std::fs::create_dir_all(cache.join("1/building-y")).unwrap();
    std::fs::create_dir_all(cache.join("7")).unwrap();

    Snapshots::new(cache.clone(), temp.path().join("jewelcase.db")).sweep(&db).await.unwrap();
    assert!(!cache.join("7").exists(), "no such library");
    assert!(!cache.join("2/building-x").exists());
    assert!(!cache.join("1/building-y").exists());
    assert!(newest(&cache.join("1")).unwrap().is_some(), "a finished file stays");
}

#[test]
fn a_database_path_is_escaped_for_a_uri() {
    assert_eq!(
        read_only_uri(Path::new("/data/a b?c#d%e/j.db")),
        "file:/data/a%20b%3Fc%23d%25e/j.db?mode=ro"
    );
}

#[tokio::test]
async fn another_librarys_churn_and_one_change_do_not_rebuild() {
    let (temp, db, app) = catalog::app().await;
    let (first, _) = built(&app).await;
    let others: Vec<_> = (3000..3050).map(|id| (Entity::Track, id, Op::Upsert)).collect();
    record(&db, 2, &others).await;
    record(&db, 1, &[(Entity::Track, 1001, Op::Upsert)]).await;

    for _ in 0..3 {
        let (status, headers, _) = fetch(&app, URI, false).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(seq(&headers), seq(&first));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let dir = temp.path().join("cache/catalog/1");
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), 2, "no build ran");
}

#[tokio::test]
async fn a_file_past_the_librarys_head_is_not_served() {
    let (temp, _db, app) = catalog::app().await;
    let (first, _) = built(&app).await;
    let dir = temp.path().join("cache/catalog/1");
    let old = newest(&dir).unwrap().unwrap();
    let name =
        |ext: &str| format!("{SCHEMA_VERSION}-{}-{}.db{ext}", seq(&first) + 1000, old.sha256);
    std::fs::rename(&old.path, dir.join(name(""))).unwrap();
    std::fs::rename(old.path.with_extension("db.gz"), dir.join(name(".gz"))).unwrap();

    let (status, ..) = fetch(&app, URI, false).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (headers, _) = built(&app).await;
    assert_eq!(seq(&headers), seq(&first));
}

#[tokio::test]
async fn the_file_has_the_device_schema_with_every_index() {
    let (_temp, _db, app) = catalog::app().await;
    let (_, bytes) = built(&app).await;
    let (_file, snapshot) = open(&bytes);
    let device = Connection::open_in_memory().unwrap();
    jewelcase_catalog::create_schema(&device).unwrap();
    let schema = |conn: &Connection| -> Vec<(String, String, Option<String>)> {
        conn.prepare("SELECT type, name, sql FROM sqlite_schema ORDER BY name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let indexes = schema(&snapshot).iter().filter(|(kind, ..)| kind == "index").count();
    assert!(indexes >= 22, "schema.sql's indexes: {indexes}");
    assert_eq!(schema(&snapshot), schema(&device));
}

#[tokio::test]
async fn a_file_a_rebuild_deleted_after_it_was_chosen_gives_way_to_its_replacement() {
    let (temp, _db, app) = catalog::app().await;
    let (headers, bytes) = built(&app).await;
    let dir = temp.path().join("cache/catalog/1");
    let gone = Current {
        seq: 1,
        sha256: "gone".to_owned(),
        path: dir.join(format!("{SCHEMA_VERSION}-1-gone.db")),
    };
    let request = owner_request("GET", URI).body(Body::empty()).unwrap();

    let response = serve(dir, gone, request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], headers[header::ETAG]);
    assert_eq!(seq(response.headers()), seq(&headers));
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body, bytes);
}

#[tokio::test]
async fn a_failed_build_is_not_started_again_until_the_cooldown_passes() {
    let (temp, _db, _app) = catalog::app().await;
    let root = temp.path().join("other");
    let dir = root.join("1");
    let snapshots = Arc::new(Snapshots::new(root.clone(), temp.path().join("jewelcase.db")));
    let attempt = async || {
        snapshots.start(1);
        eventually("the build to end", async || {
            snapshots.building.lock().unwrap().is_empty().then_some(())
        })
        .await;
    };
    // A file where the library's folder goes fails the build.
    std::fs::create_dir(&root).unwrap();
    std::fs::write(&dir, "").unwrap();
    attempt().await;
    assert!(snapshots.failed.lock().unwrap().contains_key(&1), "the first build ran and failed");
    std::fs::remove_file(&dir).unwrap();
    attempt().await;
    assert!(!dir.exists(), "no second build while the failure is recent");

    let past = Instant::now().checked_sub(RETRY_COOLDOWN).unwrap();
    snapshots.failed.lock().unwrap().insert(1, past);
    attempt().await;
    assert!(dir.exists(), "a build once the cooldown has passed");
    assert!(snapshots.failed.lock().unwrap().is_empty(), "its success lifts the cooldown");
}

#[tokio::test]
async fn a_file_of_another_schema_version_is_not_served_and_a_build_replaces_it() {
    let (temp, _db, app, snapshots) = catalog::app_with_snapshots().await;
    let (first, _) = built(&app).await;
    // A poll that saw no file before the build finished can start a second, identical build,
    // which would put the current file back after it is renamed below.
    let idle = async || {
        let idle = || snapshots.building.lock().unwrap().is_empty().then_some(());
        eventually("the builds to finish", async || idle()).await;
    };
    idle().await;
    let dir = temp.path().join("cache/catalog/1");
    let old = newest(&dir).unwrap().unwrap();
    let name = |ext: &str| format!("{}-{}-{}.db{ext}", SCHEMA_VERSION - 1, old.seq, old.sha256);
    std::fs::rename(&old.path, dir.join(name(""))).unwrap();
    std::fs::rename(old.path.with_extension("db.gz"), dir.join(name(".gz"))).unwrap();

    let (status, ..) = fetch(&app, URI, false).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "the old version's file is not served");
    let (headers, bytes) = built(&app).await;
    assert_eq!(seq(&headers), seq(&first));
    let (_file, snapshot) = open(&bytes);
    let version: i32 = snapshot.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(version, SCHEMA_VERSION);
    // The file is served before the build deletes the old ones.
    idle().await;
    let files = std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(files, 2, "the old version's files were removed by the build");
}

#[tokio::test]
async fn no_account_playlist_is_in_a_snapshot_every_account_shares() {
    let (temp, db, app) = catalog::app().await;
    add_user(&conn(&temp), 2, "user");
    db.write(|tx| {
        for (id, user) in [(5, 1), (6, 2)] {
            tx.execute(
                "INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, \
                 created_at, modified_at) VALUES (?1, 1, ?2, 'Mine', 'mine', x'01', 0, 0)",
                [id, user],
            )?;
            tx.execute("INSERT INTO playlist_items VALUES (?1, 1048576, 1, 1001, 0)", [id])?;
        }
        Ok(())
    })
    .await
    .unwrap();

    let (_, bytes) = built(&app).await;
    let (_file, snapshot) = open(&bytes);

    for table in ["playlists", "playlist_items"] {
        let rows: i64 =
            snapshot.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 0, "{table}");
    }
}
