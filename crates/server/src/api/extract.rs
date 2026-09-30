//! Request extractors that answer a bad request with a Problem, instead of axum's plain-text
//! `400`.

use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use super::Problem;

/// Deserializes the path parameters into `T`, such as `(Id, Id)`. Anything that does not fit,
/// a malformed ID above all, answers `404`, exactly like an ID out of the caller's reach
/// (`requirements/users.md` §10).
pub struct Path<T>(pub T);

impl<T: DeserializeOwned + Send, S: Send + Sync> FromRequestParts<S> for Path<T> {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Problem> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Path(value)),
            Err(_) => Err(Problem::not_found()),
        }
    }
}

/// Deserializes the query string into `T`, or answers `422 validation_failed` with serde's
/// description of what was wrong, which names only the parameter and what it expected.
pub struct Query<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequestParts<S> for Query<T> {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Problem> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Query(value)),
            Err(rejection) => Err(Problem::invalid(rejection.body_text())),
        }
    }
}

/// Deserializes a JSON body into `T`, or answers `422 validation_failed` with serde's
/// description of what was wrong.
pub struct Json<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for Json<T> {
    type Rejection = Problem;

    async fn from_request(request: Request, state: &S) -> Result<Self, Problem> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Json(value)),
            Err(rejection) => Err(Problem::invalid(rejection.body_text())),
        }
    }
}
