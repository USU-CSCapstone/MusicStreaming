//! Helpers for the API's tests: a server over a fresh database, and requests to it.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header, request};
use serde_json::Value;
use tower::ServiceExt;

use super::session::token_hash;
use super::{Images, router};
use crate::db::{Database, libraries};
use crate::plugins::Plugins;

/// The owner's token in [`app`], which [`send`] and the helpers built on it log in with.
pub const TOKEN: &str = "owner-token";

/// The API over a fresh database with setup done, and the directory that holds it. The owner,
/// user 1, is logged in on device 1 with [`TOKEN`].
pub fn app(base_path: &str) -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app_before_setup(base_path);
    // Its own connection: tests call this inside the runtime, where `write_blocking` cannot wait.
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    conn.execute(
        "INSERT INTO users (id, username, display_name, role, password, created_at, updated_at) \
         VALUES (1, 'owner', 'Owner', 'owner', '', 0, 0)",
        [],
    )
    .unwrap();
    add_device(&conn, 1, 1, TOKEN);
    (temp, db, app)
}

/// Adds account `id` with `role`, named after it, such as `user2`.
pub fn add_user(conn: &rusqlite::Connection, id: i64, role: &str) {
    conn.execute(
        "INSERT INTO users (id, username, display_name, role, password, created_at, updated_at) \
         VALUES (?1, 'user' || ?1, 'User ' || ?1, ?2, '', 0, 0)",
        rusqlite::params![id, role],
    )
    .unwrap();
}

/// Logs `user` in on a new device `id` with `token`, last seen at the epoch.
pub fn add_device(conn: &rusqlite::Connection, user: i64, id: i64, token: &str) {
    conn.execute(
        "INSERT INTO devices (id, user_id, name, type, token_hash, created_at, last_seen_at) \
         VALUES (?1, ?2, 'Test', 'desktop', ?3, 0, 0)",
        rusqlite::params![id, user, token_hash(token)],
    )
    .unwrap();
}

/// The API over a fresh database, as a new server first starts: with no owner.
pub fn app_before_setup(base_path: &str) -> (tempfile::TempDir, Arc<Database>, Router) {
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&temp.path().join("jewelcase.db")).unwrap());
    let images = Images::new(temp.path().join("cache"), "ffmpeg".into());
    // A client with a public address, unless a request carries its own `ConnectInfo`.
    let peer = SocketAddr::from(([203, 0, 113, 1], 50_000));
    let plugins = Plugins::new(db.clone(), temp.path().join("plugins"), HashMap::new());
    let app = router(base_path, db.clone(), images, Arc::new(plugins)).layer(MockConnectInfo(peer));
    (temp, db, app)
}

/// [`app`] with library 1 holding tracks 1 and 2, and library 2 holding track 3, each on
/// an album of its own library and with no lyrics or waveform.
pub async fn app_with_tracks() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app("");
    db.write(|tx| {
        libraries::create(tx, Some(1), "Music", &[Path::new("/music")], &[])?;
        libraries::create(tx, Some(2), "Other", &[Path::new("/other")], &[])?;
        tx.execute_batch(
            "INSERT INTO albums (id, library_id, title_key, artists_key, sort_key,
                                 artist_sort_key, added_at, updated_at)
             VALUES (101, 1, 'a', '', x'61', x'', 0, 0), (201, 2, 'a', '', x'61', x'', 0, 0);
             WITH t (id, library_id, album_id) AS (VALUES (1, 1, 101), (2, 1, 101), (3, 2, 201))
             INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size, file_mtime,
                                 title, sort_key, artist_sort_key, album_sort_key, codec,
                                 container, lossless, sample_rate_hz, channels, duration_us,
                                 added_at, updated_at)
             SELECT t.id, t.library_id, t.album_id, r.id, t.id || '.flac', 10, 0, 'A', x'61',
                    x'', x'61', 'flac', 'flac', 1, 44100, 2, 1000000, 0, 0
             FROM t JOIN library_roots r ON r.library_id = t.library_id;",
        )
    })
    .await
    .unwrap();
    (temp, db, app)
}

/// A request logged in as the owner, to add headers and a body to.
pub fn owner_request(method: &str, uri: &str) -> request::Builder {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
}

/// Sends a request with no body, logged in as the owner.
pub async fn send(app: Router, method: &str, uri: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let request = owner_request(method, uri).body(Body::empty()).unwrap();
    let (status, headers, body) = respond(app, request).await;
    let content_type =
        headers.get(header::CONTENT_TYPE).map(|value| value.to_str().unwrap().to_owned());
    (status, content_type, body)
}

/// The status, headers, and body of `request`'s response.
pub async fn respond(app: Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app.oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    (parts.status, parts.headers, to_bytes(body, usize::MAX).await.unwrap().to_vec())
}

/// Sends `body` as JSON with no credentials, and returns the status, headers, and JSON body of
/// the response.
pub async fn send_json(
    app: Router,
    method: &str,
    uri: &str,
    body: &Value,
) -> (StatusCode, HeaderMap, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, headers, body) = respond(app, request).await;
    (status, headers, serde_json::from_slice(&body).unwrap())
}

/// The status and JSON body of a `GET`.
pub async fn json(app: &Router, uri: &str) -> (StatusCode, Value) {
    let (status, _, body) = send(app.clone(), "GET", uri).await;
    (status, serde_json::from_slice(&body).unwrap())
}

/// `field` of every item in the list at `uri`, read a page of `limit` at a time.
pub async fn page_through(app: &Router, uri: &str, limit: u32, field: &str) -> Value {
    let mut values = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let separator = if uri.contains('?') { '&' } else { '?' };
        let mut page_uri = format!("{uri}{separator}limit={limit}");
        if let Some(cursor) = &cursor {
            page_uri += &format!("&cursor={cursor}");
        }
        let (status, page) = json(app, &page_uri).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let items = page["items"].as_array().unwrap();
        assert!(items.len() <= limit as usize);
        values.extend(items.iter().map(|item| item[field].clone()));
        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return values.into(),
        }
    }
}
