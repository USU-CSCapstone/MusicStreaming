//! The public API (`api/openapi.yaml`), served under `{basePath}/api/v1`.

mod albums;
mod artists;
mod audio;
mod authenticate;
#[cfg(test)]
mod benchmark;
mod connections;
mod credit;
pub mod cursor;
mod extract;
mod id;
mod images;
mod libraries;
mod login;
mod lyrics;
mod me;
mod origin;
mod page;
mod password;
mod playlists;
mod plays;
mod plugins;
mod problem;
mod refs;
mod search;
mod session;
mod setup;
mod sql;
#[cfg(test)]
mod testing;
mod tracks;
mod waveform;

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, FromRef};
use axum::middleware;
use axum::routing::{any, get, post, put};
use axum::{Json, Router};

pub use id::Id;
pub use images::Images;
pub use problem::{Code, Problem};
use search::Indexes;
use setup::Setup;

use crate::db::Database;
use crate::plugins::Plugins;

/// What handlers share. Each takes the part it needs, such as `State<Arc<Database>>`.
#[derive(Clone)]
struct AppState {
    db: Arc<Database>,
    images: Arc<Images>,
    indexes: Arc<Indexes>,
    setup: Arc<Setup>,
    plugins: Arc<Plugins>,
    /// Where the API is mounted, `{base_path}/api/v1`.
    prefix: Arc<str>,
}

impl FromRef<AppState> for Arc<Database> {
    fn from_ref(state: &AppState) -> Arc<Database> {
        state.db.clone()
    }
}

impl FromRef<AppState> for Arc<Indexes> {
    fn from_ref(state: &AppState) -> Arc<Indexes> {
        state.indexes.clone()
    }
}

impl FromRef<AppState> for Arc<Plugins> {
    fn from_ref(state: &AppState) -> Arc<Plugins> {
        state.plugins.clone()
    }
}

impl FromRef<AppState> for Arc<Images> {
    fn from_ref(state: &AppState) -> Arc<Images> {
        state.images.clone()
    }
}

/// The API's routes, nested under `{base_path}/api/v1`.
pub fn router(base_path: &str, db: Arc<Database>, images: Images, plugins: Arc<Plugins>) -> Router {
    let prefix = format!("{base_path}/api/v1");
    let state = AppState {
        db,
        images: Arc::new(images),
        indexes: Arc::default(),
        setup: Arc::default(),
        plugins,
        prefix: prefix.as_str().into(),
    };
    // These answer `401` without a token.
    let signed_in = Router::new()
        .route("/auth/logout", post(session::logout))
        .route("/me", get(me::get))
        .route("/me/plays", post(plays::record))
        .route("/me/plugins", get(connections::list))
        .route(
            "/me/plugins/{plugin_id}/settings",
            get(connections::get_settings)
                .put(connections::set_settings)
                .delete(connections::disconnect),
        )
        .route("/libraries", get(libraries::list))
        .route("/libraries/{library_id}", get(libraries::get))
        .route("/libraries/{library_id}/albums", get(albums::list))
        .route("/libraries/{library_id}/artists", get(artists::list))
        .route("/libraries/{library_id}/playlists", get(playlists::list))
        .route("/libraries/{library_id}/tracks", get(tracks::list))
        .route("/libraries/{library_id}/tracks/{track_id}", get(tracks::get))
        .route("/libraries/{library_id}/search", get(search::search))
        .route("/libraries/{library_id}/images/{image_id}", get(images::get))
        .route("/libraries/{library_id}/tracks/{track_id}/lyrics", get(lyrics::get))
        .route("/libraries/{library_id}/tracks/{track_id}/waveform", get(waveform::get))
        .route("/libraries/{library_id}/tracks/{track_id}/playback", get(audio::playback))
        .route("/libraries/{library_id}/tracks/{track_id}/audio", get(audio::audio))
        .route("/libraries/{library_id}/artists/{artist_id}", get(artists::get))
        .route("/libraries/{library_id}/albums/{album_id}", get(albums::get))
        .route(
            "/admin/plugins",
            // A plugin file may be larger than the default limit on a request body.
            get(plugins::list)
                .post(plugins::install)
                .layer(DefaultBodyLimit::max(jewelcase_plugins::MAX_SIZE)),
        )
        .route("/admin/plugins/{plugin_id}", get(plugins::get).delete(plugins::uninstall))
        .route("/admin/plugins/{plugin_id}/permissions", put(plugins::set_permissions))
        .route("/admin/plugins/{plugin_id}/libraries/{library_id}", put(plugins::set_enabled))
        .route("/admin/plugins/{plugin_id}/run", post(plugins::run))
        .route(
            "/admin/plugins/{plugin_id}/settings",
            get(plugins::get_settings).put(plugins::set_settings),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate::require));
    // Until setup is done, these answer `503 setup_required`.
    let after_setup = Router::new()
        .route("/auth/login", post(login::login))
        .merge(signed_in)
        .route_layer(middleware::from_fn_with_state(state.clone(), setup::require));
    // Anything else under the API is a Problem, like every other error.
    let api = Router::new()
        .route("/health", get(health))
        .route("/server", get(setup::server_info))
        .route("/setup", post(setup::complete))
        .merge(after_setup)
        .method_not_allowed_fallback(method_not_allowed)
        .fallback(not_found)
        .with_state(state);
    Router::new()
        .nest(&prefix, api)
        // `nest` leaves out the prefix with a trailing slash.
        .route(&format!("{prefix}/"), any(not_found))
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn not_found() -> Problem {
    Problem::not_found()
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
            assert_eq!(content_type.as_deref(), Some("application/problem+json"), "{uri}");
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
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["code"], "method_not_allowed");
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
