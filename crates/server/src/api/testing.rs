//! Helpers for the API's tests: a server over a fresh database, and requests to it.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header, request};
use serde_json::Value;
use tower::ServiceExt;

use super::session::token_hash;
use super::{AppState, Images, router};
use crate::db::Database;
use crate::db::testing::{Album, Device, Track, User, library};
use crate::plugins::Plugins;

pub mod catalog;
pub mod player;
pub mod socket;

/// The owner's token in [`app`], which [`send`] and the helpers built on it log in with.
pub const TOKEN: &str = "owner-token";

/// The API over a fresh database with setup done, and the directory that holds it. The owner,
/// user 1, is logged in on device 1 with [`TOKEN`].
pub fn app(base_path: &str) -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app_before_setup(base_path);
    // Its own connection: tests call this inside the runtime, where `write_blocking` cannot wait.
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    let owner =
        User { username: "owner".into(), display_name: "Owner".into(), ..User::new(1, "owner") };
    owner.insert(&conn).unwrap();
    add_device(&conn, 1, 1, TOKEN);
    (temp, db, app)
}

/// Adds account `id` with `role`, named after it, such as `user2`.
pub fn add_user(conn: &rusqlite::Connection, id: i64, role: &'static str) {
    User::new(id, role).insert(conn).unwrap();
}

/// Logs `user` in on a new device `id` with `token`, last seen at the epoch.
pub fn add_device(conn: &rusqlite::Connection, user: i64, id: i64, token: &str) {
    Device::new(user, id, token_hash(token)).insert(conn).unwrap();
}

/// The API over a fresh database, as a new server first starts: with no owner.
pub fn app_before_setup(base_path: &str) -> (tempfile::TempDir, Arc<Database>, Router) {
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&temp.path().join("jewelcase.db")).unwrap());
    let images = Images::new(temp.path().join("cache"), "ffmpeg".into());
    // A client with a public address, unless a request carries its own `ConnectInfo`.
    let peer = SocketAddr::from(([203, 0, 113, 1], 50_000));
    let plugins = Plugins::new(db.clone(), temp.path().join("plugins"), HashMap::new());
    let app = router(base_path, db.clone(), images, Arc::new(plugins), Arc::default())
        .layer(MockConnectInfo(peer));
    (temp, db, app)
}

/// What the API's handlers share, over `db` in `temp`, mounted at the root.
pub fn state(temp: &tempfile::TempDir, db: Arc<Database>) -> AppState {
    let images = Images::new(temp.path().join("cache"), "ffmpeg".into());
    let plugins = Plugins::new(db.clone(), temp.path().join("plugins"), HashMap::new());
    AppState::new(db, images, Arc::new(plugins), Arc::default(), "/api/v1")
}

/// [`app`] with library 1 holding tracks 1 and 2, and library 2 holding track 3, each called
/// `A`, on an album of its own library, and with no lyrics or waveform.
pub async fn app_with_tracks() -> (tempfile::TempDir, Arc<Database>, Router) {
    app_with_tracks_at("").await
}

/// [`app_with_tracks`] mounted under `base_path`.
pub async fn app_with_tracks_at(base_path: &str) -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app(base_path);
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        library(tx, 2, "Other", "/other")?;
        Album::new(101, 1).insert(tx)?;
        Album::new(201, 2).insert(tx)?;
        for (id, library, album) in [(1, 1, 101), (2, 1, 101), (3, 2, 201)] {
            let track = Track::new(id, library, album);
            Track { title: "A".into(), sort_key: b"a".to_vec(), ..track }.insert(tx)?;
        }
        Ok(())
    })
    .await
    .unwrap();
    (temp, db, app)
}

/// Runs ffmpeg with `args` to make a test file. Panics if ffmpeg is missing or fails.
pub fn ffmpeg(args: &[&str]) {
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success());
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

/// Polls `check` every 10 ms until it returns a value, panicking with `what` after 30 seconds:
/// long enough for background work on a loaded machine, so only work that never happens fails.
pub async fn eventually<T>(what: &str, mut check: impl AsyncFnMut() -> Option<T>) -> T {
    let polled = async {
        loop {
            if let Some(found) = check().await {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(30), polled)
        .await
        .unwrap_or_else(|_| panic!("{what}: timed out after 30 s"))
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
