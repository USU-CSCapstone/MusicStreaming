//! The public API (`api/openapi.yaml`), served under `{basePath}/api/v1`.

mod albums;
mod artists;
mod audio;
pub mod cursor;
mod id;
mod images;
mod libraries;
mod lyrics;
mod page;
mod playlists;
mod problem;
mod query;
mod refs;
mod search;
#[cfg(test)]
mod testing;
mod tracks;
mod waveform;

use std::sync::Arc;

use axum::extract::FromRef;
use axum::routing::{any, get};
use axum::{Json, Router};

pub use id::Id;
pub use images::Images;
pub use problem::{Code, Problem};
use search::Search;

use crate::db::Database;

/// What handlers share. Each takes the part it needs, such as `State<Arc<Database>>`.
#[derive(Clone)]
struct AppState {
    db: Arc<Database>,
    images: Arc<Images>,
    search: Arc<Search>,
}

impl FromRef<AppState> for Arc<Database> {
    fn from_ref(state: &AppState) -> Arc<Database> {
        state.db.clone()
    }
}

impl FromRef<AppState> for Arc<Search> {
    fn from_ref(state: &AppState) -> Arc<Search> {
        state.search.clone()
    }
}

impl FromRef<AppState> for Arc<Images> {
    fn from_ref(state: &AppState) -> Arc<Images> {
        state.images.clone()
    }
}

/// The API's routes, nested under `{base_path}/api/v1`.
pub fn router(base_path: &str, db: Arc<Database>, images: Images) -> Router {
    let prefix = format!("{base_path}/api/v1");
    // Anything else under the API is a Problem, like every other error.
    let api = Router::new()
        .route("/health", get(health))
        .route("/libraries", get(libraries::list))
        .route("/libraries/{library_id}", get(libraries::get))
        .route("/libraries/{library_id}/albums", get(albums::list))
        .route("/libraries/{library_id}/artists", get(artists::list))
        .route("/libraries/{library_id}/playlists", get(playlists::list))
        .route("/libraries/{library_id}/tracks", get(tracks::list))
        .route(
            "/libraries/{library_id}/tracks/{track_id}",
            get(tracks::get),
        )
        .route("/libraries/{library_id}/search", get(search::search))
        .route(
            "/libraries/{library_id}/images/{image_id}",
            get(images::get),
        )
        .route(
            "/libraries/{library_id}/tracks/{track_id}/lyrics",
            get(lyrics::get),
        )
        .route(
            "/libraries/{library_id}/tracks/{track_id}/waveform",
            get(waveform::get),
        )
        .route(
            "/libraries/{library_id}/tracks/{track_id}/playback",
            get(audio::playback),
        )
        .route(
            "/libraries/{library_id}/tracks/{track_id}/audio",
            get(audio::audio),
        )
        .route(
            "/libraries/{library_id}/artists/{artist_id}",
            get(artists::get),
        )
        .route(
            "/libraries/{library_id}/albums/{album_id}",
            get(albums::get),
        )
        .method_not_allowed_fallback(method_not_allowed)
        .fallback(not_found)
        .with_state(AppState {
            db,
            images: Arc::new(images),
            search: Arc::default(),
        });
    Router::new()
        .nest(&prefix, api)
        // `nest` leaves out the prefix with a trailing slash.
        .route(&format!("{prefix}/"), any(not_found))
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn not_found() -> Problem {
    Problem::new(Code::NotFound)
}

async fn method_not_allowed() -> Problem {
    Problem::new(Code::MethodNotAllowed)
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use serde_json::{Value, json};

    use super::testing::{app, send};
    use super::*;
    use crate::db::DbError;

    #[tokio::test]
    async fn unknown_paths_are_not_found_problems() {
        let (_temp, _db, app) = app("");
        for uri in ["/api/v1/nowhere", "/api/v1/", "/api/v1"] {
            let (status, content_type, body) = send(app.clone(), "GET", uri).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(
                content_type.as_deref(),
                Some("application/problem+json"),
                "{uri}"
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                json!({ "type": "about:blank", "title": "Not Found", "status": 404, "code": "not_found" })
            );
        }
    }

    #[tokio::test]
    async fn unsupported_methods_are_method_not_allowed_problems() {
        let (_temp, _db, app) = app("");
        let (status, content_type, body) = send(app, "POST", "/api/v1/health").await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(content_type.as_deref(), Some("application/problem+json"));
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["code"],
            "method_not_allowed"
        );
    }

    #[tokio::test]
    async fn the_api_is_mounted_under_the_base_path() {
        let (_temp, _db, app) = app("/music");
        let (status, _, _) = send(app.clone(), "GET", "/music/api/v1/health").await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = send(app, "GET", "/api/v1/health").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn database_errors_are_internal_problems_without_their_cause() {
        let problem = Problem::from(DbError::Sqlite(rusqlite::Error::QueryReturnedNoRows));
        let response = problem.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({ "type": "about:blank", "title": "Internal Server Error", "status": 500, "code": "internal" })
        );
    }
}
