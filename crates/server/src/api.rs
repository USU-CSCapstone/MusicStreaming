//! The public API (`api/openapi.yaml`), served under `{basePath}/api/v1`.

mod albums;
mod artists;
mod audio;
mod authenticate;
mod connections;
mod credit;
mod cursor;
mod devices;
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
mod tag_filter;
mod tags;
#[cfg(test)]
pub(crate) mod testing;
mod time;
mod tracks;
mod waveform;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{DefaultBodyLimit, FromRef};
use axum::middleware;
use axum::routing::{MethodRouter, any, get, post};
use tower_http::timeout::RequestBodyDeadlineLayer;

pub use id::{AnyId, Id, Uuid};
pub use images::Images;
pub use player::Players;
pub use problem::{Code, Problem};
pub use realtime::Hub;
pub use scope::Caller;
pub use search::Indexes;
use setup::Setup;

use crate::config::TrustedProxies;
use crate::db::Database;
use crate::plugins::Plugins;

/// The largest body a client may send before it signs in: ample for a name, a password and a
/// device.
const SIGNED_OUT_BODY_LIMIT: usize = 64 * 1024;

/// How long a client that has not signed in has to send its whole body, however steadily it
/// trickles in.
const SIGNED_OUT_BODY_DEADLINE: Duration = Duration::from_secs(30);

/// What handlers share. Each takes the part it needs, such as `State<Arc<Database>>`.
#[derive(Clone, FromRef)]
pub struct AppState {
    pub db: Arc<Database>,
    pub images: Arc<Images>,
    pub indexes: Arc<Indexes>,
    /// Whether setup is seen done; a default one has not seen it yet.
    pub setup: Arc<Setup>,
    pub plugins: Arc<Plugins>,
    /// The open realtime connections.
    pub hub: Arc<Hub>,
    /// Each account's playback state.
    pub players: Arc<Players>,
    /// Where the API is mounted, as [`prefix`] gives it.
    pub prefix: Arc<str>,
    /// The proxies whose forwarded headers are believed ([`origin::Origin`]).
    pub trusted_proxies: Arc<TrustedProxies>,
}

/// Where the API is mounted under `base_path`: `{base_path}/api/v1`.
pub fn prefix(base_path: &str) -> Arc<str> {
    format!("{base_path}/api/v1").into()
}

/// The API's routes, nested under `state`'s prefix.
pub fn router(state: AppState) -> Router {
    let prefix = state.prefix.clone();
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
        .route("/auth/login", signed_out(post(login::login)))
        .merge(signed_in)
        .merge(admin)
        .route_layer(middleware::from_fn_with_state(state.clone(), setup::require));
    // Anything else under the API is a Problem, like every other error.
    let api = Router::new()
        .route("/health", get(health::health))
        .route("/server", get(setup::server_info))
        .route("/setup", signed_out(post(setup::complete)))
        .merge(after_setup)
        .method_not_allowed_fallback(method_not_allowed)
        .fallback(not_found)
        .with_state(state);
    Router::new()
        .nest(&prefix, api)
        // `nest` leaves out the prefix with a trailing slash.
        .route(&format!("{prefix}/"), any(not_found))
}

/// `route` with a small body limit and a deadline for the whole body, for a route anyone may
/// call: past either, its JSON body is refused as `422 validation_failed`.
fn signed_out(route: MethodRouter<AppState>) -> MethodRouter<AppState> {
    route.layer((
        DefaultBodyLimit::max(SIGNED_OUT_BODY_LIMIT),
        RequestBodyDeadlineLayer::new(SIGNED_OUT_BODY_DEADLINE),
    ))
}

/// A `404 not_found` Problem, for a path the API does not have.
async fn not_found() -> Problem {
    Problem::not_found()
}

/// A `405 method_not_allowed` Problem, for a path the API has under other methods.
async fn method_not_allowed() -> Problem {
    Problem::new(Code::MethodNotAllowed)
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use axum::body::{Body, Bytes, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::response::IntoResponse;
    use futures_util::stream;
    use serde_json::{Value, json};
    use tokio::time::Instant;

    use super::testing::{app, app_before_setup, respond, send, send_json};
    use super::*;
    use crate::db::DbError;

    mod benchmark;
    mod end_to_end;
    mod spec;

    #[tokio::test]
    async fn a_large_body_is_refused_before_sign_in() {
        let (_temp, _db, app) = app("");
        let (_temp, _db, new_server) = app_before_setup("");
        let body = json!({ "username": "a".repeat(SIGNED_OUT_BODY_LIMIT), "password": "p" });
        for (app, uri) in [(app, "/api/v1/auth/login"), (new_server, "/api/v1/setup")] {
            let (status, _, problem) = send_json(app, "POST", uri, &body).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
            assert!(problem["detail"].as_str().unwrap().contains("length limit"), "{problem}");
        }
    }

    /// A client that sends a space every 5 s, never pausing long enough for a gap between
    /// frames to time out, and finishes after ten minutes.
    #[tokio::test(start_paused = true)]
    async fn a_trickling_body_is_cut_off_before_sign_in() {
        let (_temp, _db, app) = app("");
        let (_temp, _db, new_server) = app_before_setup("");
        for (app, uri) in [(app, "/api/v1/auth/login"), (new_server, "/api/v1/setup")] {
            let spaces = stream::unfold(0, |sent| async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                (sent < 120).then(|| (Ok::<_, Infallible>(Bytes::from_static(b" ")), sent + 1))
            });
            let request = Request::post(uri).header("content-type", "application/json");
            let sent = Instant::now();
            let (status, _, _) =
                respond(app, request.body(Body::from_stream(spaces)).unwrap()).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
            let waited = sent.elapsed();
            assert!(waited >= SIGNED_OUT_BODY_DEADLINE, "{uri}: {waited:?}");
            assert!(waited < SIGNED_OUT_BODY_DEADLINE * 2, "{uri}: {waited:?}");
        }
    }

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
