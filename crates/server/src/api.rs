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
mod devices;
#[cfg(test)]
mod end_to_end;
mod extract;
mod health;
mod id;
mod images;
mod libraries;
mod login;
mod lyrics;
mod me;
mod origin;
mod page;
mod password;
mod player;
mod playlists;
mod plays;
mod plugins;
mod problem;
mod realtime;
mod recent_searches;
mod refs;
mod routes;
mod scope;
mod search;
mod session;
mod setup;
#[cfg(test)]
pub(crate) mod testing;
mod time;
mod tracks;
mod waveform;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::middleware;
use axum::routing::{any, get, post};

pub use id::{AnyId, Id, Uuid};
pub use images::Images;
pub use player::Players;
pub use problem::{Code, Problem};
use realtime::Hub;
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
    /// The open realtime connections.
    hub: Arc<Hub>,
    /// Each account's playback state.
    players: Arc<Players>,
    /// Where the API is mounted, `{base_path}/api/v1`.
    prefix: Arc<str>,
}

impl AppState {
    /// The state of an API mounted at `prefix`, before any search index is built, setup is
    /// seen done, or a realtime connection opens.
    fn new(
        db: Arc<Database>,
        images: Images,
        plugins: Arc<Plugins>,
        players: Arc<Players>,
        prefix: &str,
    ) -> AppState {
        AppState {
            db,
            images: Arc::new(images),
            indexes: Arc::default(),
            setup: Arc::default(),
            plugins,
            hub: Arc::default(),
            players,
            prefix: prefix.into(),
        }
    }
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

impl FromRef<AppState> for Arc<Hub> {
    fn from_ref(state: &AppState) -> Arc<Hub> {
        state.hub.clone()
    }
}

impl FromRef<AppState> for Arc<Players> {
    fn from_ref(state: &AppState) -> Arc<Players> {
        state.players.clone()
    }
}

impl FromRef<AppState> for Arc<Images> {
    fn from_ref(state: &AppState) -> Arc<Images> {
        state.images.clone()
    }
}

/// The API's routes, nested under `{base_path}/api/v1`.
pub fn router(
    base_path: &str,
    db: Arc<Database>,
    images: Images,
    plugins: Arc<Plugins>,
    players: Arc<Players>,
) -> Router {
    let prefix = format!("{base_path}/api/v1");
    let state = AppState::new(db, images, plugins, players, &prefix);
    // These answer `401` without a token.
    let signed_in = Router::new()
        .route("/auth/logout", post(session::logout))
        .route("/realtime", get(realtime::open))
        .merge(routes::me::router())
        .merge(routes::libraries::router())
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate::require));
    // These answer as signed-in routes do, and `403` to anyone but an admin or the owner.
    let admin = routes::admin::router()
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate::require_admin));
    // Until setup is done, these answer `503 setup_required`.
    let after_setup = Router::new()
        .route("/auth/login", post(login::login))
        .merge(signed_in)
        .merge(admin)
        .route_layer(middleware::from_fn_with_state(state.clone(), setup::require));
    // Anything else under the API is a Problem, like every other error.
    let api = Router::new()
        .route("/health", get(health::health))
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
