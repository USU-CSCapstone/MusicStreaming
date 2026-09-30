//! Libraries: `listLibraries` and `getLibrary` (`api/openapi.yaml`).
//!
//! Each shows only the libraries the caller reaches (`requirements/users.md` §5): `getLibrary`
//! through `authenticate`, like every path under a library, and `listLibraries` here.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::{OptionalExtension, Row};
use serde::Serialize;

use super::authenticate::Caller;
use super::extract::Path;
use super::{Id, Problem};
use crate::db::Database;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    id: Id,
    name: String,
    track_count: i64,
    album_count: i64,
    artist_count: i64,
    duration_us: i64,
    /// Content is still appearing: a scan is queued or running.
    scanning: bool,
}

#[derive(Serialize)]
pub struct Libraries {
    items: Vec<Library>,
}

/// The counts are kept current by the scanner, so a library reads as one row.
const SELECT: &str = "SELECT id, name, track_count, album_count, artist_count, duration_us, \
     EXISTS (SELECT 1 FROM scans WHERE scans.library_id = libraries.id \
             AND state IN ('queued', 'running')) \
     FROM libraries";

/// Oldest first: with no default library set, clients open the first one (`AccountSettings`).
pub async fn list(
    State(db): State<Arc<Database>>,
    caller: Caller,
) -> Result<Json<Libraries>, Problem> {
    let items = db
        .read(move |conn| {
            conn.prepare_cached(&format!(
                "{SELECT} WHERE ?1 OR id IN (SELECT library_id FROM library_access \
                 WHERE user_id = ?2) ORDER BY created_at, id"
            ))?
            .query_map(rusqlite::params![caller.reaches_all, caller.user], library)?
            .collect()
        })
        .await?;
    Ok(Json(Libraries { items }))
}

pub async fn get(
    State(db): State<Arc<Database>>,
    Path(Id(id)): Path<Id>,
) -> Result<Json<Library>, Problem> {
    db.read(move |conn| {
        conn.prepare_cached(&format!("{SELECT} WHERE id = ?1"))?.query_row([id], library).optional()
    })
    .await?
    .map(Json)
    .ok_or_else(Problem::not_found)
}

fn library(row: &Row) -> rusqlite::Result<Library> {
    Ok(Library {
        id: Id(row.get(0)?),
        name: row.get(1)?,
        track_count: row.get(2)?,
        album_count: row.get(3)?,
        artist_count: row.get(4)?,
        duration_us: row.get(5)?,
        scanning: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};

    use super::super::testing::{add_device, add_user, app, app_with_tracks, respond, send};
    use crate::db::libraries;

    #[tokio::test]
    async fn lists_and_gets_libraries() {
        let (_temp, db, app) = app("");
        db.write(|tx| {
            // Created in the order 3, 1, 2: neither the name order nor the id order.
            for (id, name, created_at) in
                [(1, "Cassettes", 20), (2, "Archive", 30), (3, "Bootlegs", 10)]
            {
                libraries::create(tx, Some(id), name, &[], &[])?;
                tx.execute("UPDATE libraries SET created_at = ?2 WHERE id = ?1", [id, created_at])?;
            }
            tx.execute(
                "UPDATE libraries SET track_count = 12, album_count = 1, artist_count = 1, \
                 duration_us = 2400000000 WHERE id = 1",
                [],
            )?;
            tx.execute(
                "INSERT INTO scans (id, library_id, trigger, state, scopes, created_at) \
                 VALUES (7, 2, 'initial', 'running', '[]', 0)",
                [],
            )
        })
        .await
        .unwrap();

        let (status, _, body) = send(app.clone(), "GET", "/api/v1/libraries").await;
        assert_eq!(status, StatusCode::OK);
        let items = serde_json::from_slice::<Value>(&body).unwrap()["items"].clone();
        let ids: Vec<&str> = items
            .as_array()
            .unwrap()
            .iter()
            .map(|library| library["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["3", "1", "2"], "oldest first");
        assert_eq!(
            items[1],
            json!({ "id": "1", "name": "Cassettes", "trackCount": 12, "albumCount": 1,
                    "artistCount": 1, "durationUs": 2_400_000_000_i64, "scanning": false })
        );
        assert_eq!(items[2]["scanning"], true, "a running scan");

        let (status, _, body) = send(app, "GET", "/api/v1/libraries/2").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["name"], "Archive");
    }

    #[tokio::test]
    async fn lists_only_the_libraries_the_caller_reaches() {
        let (temp, _db, app) = app_with_tracks().await;
        let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
        for (id, role) in [(2, "user"), (3, "user"), (4, "admin")] {
            add_user(&conn, id, role);
            add_device(&conn, id, id, &format!("token-{id}"));
        }
        conn.execute("INSERT INTO library_access VALUES (2, 2, 0)", []).unwrap();

        let ids = |token: &'static str| {
            let app = app.clone();
            async move {
                let request = Request::get("/api/v1/libraries")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap();
                let (_, _, body) = respond(app, request).await;
                let items = serde_json::from_slice::<Value>(&body).unwrap()["items"].clone();
                items.as_array().unwrap().iter().map(|l| l["id"].clone()).collect::<Vec<_>>()
            }
        };
        assert_eq!(ids("token-2").await, ["2"], "granted one");
        // No access is an empty list, not an error (`requirements/users.md` §5).
        assert!(ids("token-3").await.is_empty(), "granted none");
        assert_eq!(ids("token-4").await, ["1", "2"], "an admin");
        assert_eq!(ids("owner-token").await, ["1", "2"], "the owner");
    }

    #[tokio::test]
    async fn an_unknown_or_malformed_library_is_not_found() {
        let (_temp, _db, app) = app("");
        for uri in ["/api/v1/libraries/1", "/api/v1/libraries/01", "/api/v1/libraries/x"] {
            let (status, content_type, _) = send(app.clone(), "GET", uri).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(content_type.as_deref(), Some("application/problem+json"), "{uri}");
        }
    }
}
