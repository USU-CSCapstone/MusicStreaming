//! The web app: SvelteKit's static build (`design/general.md` §7.1), served beside the API.
//!
//! A path that is not a file gets `index.html`, and the app routes it in the browser. Files
//! under `_app/immutable` have content hashes in their names, so they cache forever; anything
//! else, `index.html` above all, is checked on every load so a new version shows at once.

use std::path::Path;

use axum::Router;
use axum::http::{HeaderValue, header};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

/// Where the container image puts the build.
pub const DIR: &str = "/usr/share/jewelcase/web";

/// Serves the build in `dir` at the root.
pub fn router(dir: &Path) -> Router {
    let cache = |value| {
        SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static(value))
    };
    let immutable = Router::new()
        .nest_service("/_app/immutable", ServeDir::new(dir.join("_app/immutable")))
        .layer(cache("public, max-age=31536000, immutable"));
    let app = Router::new()
        .fallback_service(ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html"))))
        .layer(cache("no-cache"));
    immutable.merge(app)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;
    use crate::api::{self, Images};
    use crate::db::Database;

    /// The API with the web app beside it, as `run` serves them, and a build of three files.
    fn app() -> (tempfile::TempDir, Router) {
        let temp = tempfile::tempdir().unwrap();
        let web = temp.path().join("web");
        std::fs::create_dir_all(web.join("_app/immutable")).unwrap();
        std::fs::write(web.join("index.html"), "<html>app</html>").unwrap();
        std::fs::write(web.join("robots.txt"), "robots").unwrap();
        std::fs::write(web.join("_app/immutable/entry.abc.js"), "js").unwrap();
        let db = Arc::new(Database::open(&temp.path().join("jewelcase.db")).unwrap());
        let images = Images::new(temp.path().join("cache"), "ffmpeg".into());
        let app = api::router("", db, images).merge(router(&web));
        (temp, app)
    }

    /// The status, `Cache-Control`, and body of a `GET`.
    async fn get(app: &Router, uri: &str) -> (StatusCode, Option<String>, String) {
        let request = Request::get(uri).body(Body::empty()).unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let cache = response
            .headers()
            .get(header::CACHE_CONTROL)
            .map(|value| value.to_str().unwrap().to_owned());
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, cache, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn serves_files_and_routes_everything_else_to_the_app() {
        let (_temp, app) = app();
        let no_cache = Some("no-cache".to_owned());
        for (uri, body) in [
            ("/", "<html>app</html>"),
            ("/albums/123", "<html>app</html>"),
            ("/robots.txt", "robots"),
            // Nothing outside the build is reachable.
            ("/../jewelcase.db", "<html>app</html>"),
        ] {
            assert_eq!(
                get(&app, uri).await,
                (StatusCode::OK, no_cache.clone(), body.to_owned()),
                "{uri}"
            );
        }
    }

    #[tokio::test]
    async fn hashed_files_cache_forever() {
        let (_temp, app) = app();
        let (status, cache, body) = get(&app, "/_app/immutable/entry.abc.js").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(cache.as_deref(), Some("public, max-age=31536000, immutable"));
        assert_eq!(body, "js");
        // A stale page asking for a file an update removed gets a 404, not the app.
        let (status, _, _) = get(&app, "/_app/immutable/entry.old.js").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_api_keeps_its_own_answers() {
        let (_temp, app) = app();
        let (status, cache, body) = get(&app, "/api/v1/nowhere").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(cache, None);
        assert!(body.contains("not_found"), "{body}");
        let (status, _, body) = get(&app, "/api/v1/health").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("ok"), "{body}");
    }
}
