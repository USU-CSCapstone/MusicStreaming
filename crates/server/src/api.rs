//! The public API (`api/openapi.yaml`), served under `{basePath}/api/v1`.

pub mod cursor;
mod id;
mod problem;

use axum::routing::{any, get};
use axum::{Json, Router};

pub use id::Id;
pub use problem::{Code, Problem};

/// The API's routes, nested under `{base_path}/api/v1`.
pub fn router(base_path: &str) -> Router {
    let prefix = format!("{base_path}/api/v1");
    // Anything else under the API is a Problem, like every other error.
    let api = Router::new()
        .route("/health", get(health))
        .method_not_allowed_fallback(method_not_allowed)
        .fallback(not_found);
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
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use axum::response::IntoResponse;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::db::DbError;

    async fn send(app: Router, method: &str, uri: &str) -> (StatusCode, Option<String>, Vec<u8>) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|value| value.to_str().unwrap().to_owned());
        (
            response.status(),
            content_type,
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
    }

    #[tokio::test]
    async fn unknown_paths_are_not_found_problems() {
        for uri in ["/api/v1/nowhere", "/api/v1/", "/api/v1"] {
            let (status, content_type, body) = send(router(""), "GET", uri).await;
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
        let (status, content_type, body) = send(router(""), "POST", "/api/v1/health").await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(content_type.as_deref(), Some("application/problem+json"));
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["code"],
            "method_not_allowed"
        );
    }

    #[tokio::test]
    async fn the_api_is_mounted_under_the_base_path() {
        let (status, _, _) = send(router("/music"), "GET", "/music/api/v1/health").await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = send(router("/music"), "GET", "/api/v1/health").await;
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
