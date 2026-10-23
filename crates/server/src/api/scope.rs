//! The library a request is about, and whose request it is.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use super::Problem;

/// The caller's account and the library the request's path names, which that account reaches
/// (`design/general.md` §6). Only the gate in [`super::authenticate`] makes one, once it has
/// checked that reach, so a handler that takes its library from here serves only a library the
/// caller reaches. Any handler whose path names a `library_id` takes it in place of that ID.
#[derive(Debug, Clone, Copy)]
pub struct Scope {
    pub user: i64,
    pub library: i64,
}

impl<S: Send + Sync> FromRequestParts<S> for Scope {
    type Rejection = Problem;

    /// The scope the gate made, or `404` if it made none: a route whose path names no
    /// `library_id`, or one outside the gate, reaches no library.
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Scope, Problem> {
        parts.extensions.get::<Scope>().copied().ok_or_else(Problem::not_found)
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{Request, StatusCode};
    use axum::response::IntoResponse;

    use super::*;

    #[tokio::test]
    async fn a_request_the_gate_made_no_scope_for_reaches_no_library() {
        let (mut parts, ()) = Request::new(()).into_parts();
        let refused = Scope::from_request_parts(&mut parts, &()).await.unwrap_err();
        assert_eq!(refused.into_response().status(), StatusCode::NOT_FOUND);
    }
}
