//! Query strings, with a bad one answered as a Problem instead of axum's plain-text `400`.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use super::{Code, Problem};

/// Deserializes the query string into `T`, or answers `422 validation_failed` with serde's
/// description of what was wrong, which names only the parameter and what it expected.
pub struct Query<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequestParts<S> for Query<T> {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Problem> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Query(value)),
            Err(rejection) => {
                Err(Problem::new(Code::ValidationFailed).detail(rejection.body_text()))
            }
        }
    }
}
