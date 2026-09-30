//! Playlists: `listPlaylists` (`api/openapi.yaml`).
//!
//! A playlist belongs to a user, and there are no accounts yet, so every library lists none.
//! The rest of the playlist API arrives with accounts; until then any playlist's ID answers
//! `404` like an unknown path.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use rusqlite::OptionalExtension;
use serde::Deserialize;

use super::page::{self, Order, Page};
use super::query::Query;
use super::{Code, Id, Problem};
use crate::db::Database;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum PlaylistSort {
    Name,
    Created,
    #[default]
    Modified,
    PlayCount,
}

/// The spec's parameters, checked like every other list's so a client's mistakes show now.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[expect(
    dead_code,
    reason = "with no playlists, the order and filter apply to nothing"
)]
pub struct ListQuery {
    #[serde(default)]
    sort: PlaylistSort,
    #[serde(default)]
    order: Order,
    q: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// An empty page. Its items have no type yet, so `()` stands in for one.
pub async fn list(
    State(db): State<Arc<Database>>,
    Path(library_id): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<()>>, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    page::limit(query.limit)?;
    // An empty list gives out no cursors, so no cursor is one of ours.
    if query.cursor.is_some() {
        return Err(Problem::new(Code::ValidationFailed).detail("cursor is not valid"));
    }
    db.read(move |conn| {
        conn.prepare_cached("SELECT 1 FROM libraries WHERE id = ?1")?
            .query_row([library], |_| Ok(()))
            .optional()
    })
    .await?
    .ok_or_else(|| Problem::new(Code::NotFound))?;
    Ok(Json(Page {
        items: Vec::new(),
        next_cursor: None,
        total: 0,
    }))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use super::super::testing::{app_with_tracks, send};

    #[tokio::test]
    async fn every_library_has_none() {
        let (_temp, _db, app) = app_with_tracks().await;
        for uri in [
            "/api/v1/libraries/1/playlists",
            "/api/v1/libraries/2/playlists?sort=name&order=desc&q=road&limit=10",
        ] {
            let (status, _, body) = send(app.clone(), "GET", uri).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                body,
                json!({ "items": [], "nextCursor": null, "total": 0 }),
                "{uri}"
            );
        }
    }

    #[tokio::test]
    async fn problems() {
        let (_temp, _db, app) = app_with_tracks().await;
        for (uri, status) in [
            ("/api/v1/libraries/9/playlists", StatusCode::NOT_FOUND),
            ("/api/v1/libraries/1/playlists/1", StatusCode::NOT_FOUND),
            (
                "/api/v1/libraries/1/playlists?cursor=00",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "/api/v1/libraries/1/playlists?sort=size",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "/api/v1/libraries/1/playlists?limit=0",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "/api/v1/libraries/1/playlists?pinned=true",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            let (actual, content_type, _) = send(app.clone(), "GET", uri).await;
            assert_eq!(actual, status, "{uri}");
            assert_eq!(
                content_type.as_deref(),
                Some("application/problem+json"),
                "{uri}"
            );
        }
    }
}
