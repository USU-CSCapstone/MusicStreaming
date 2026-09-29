//! Errors as RFC 9457 problem details: the spec's `Problem` (`api/openapi.yaml`).
//!
//! Every error the API returns is one of these. Handlers return `Result<T, Problem>`, and `?`
//! turns a database error into a `500 internal` whose cause is logged, never sent.

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::db::DbError;

/// The spec's `Problem.code`: what went wrong, for clients to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    Unauthenticated,
    SessionRevoked,
    InvalidCredentials,
    RateLimited,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    ValidationFailed,
    WeakPassword,
    BreachedPassword,
    UsernameTaken,
    VersionConflict,
    CapacityExceeded,
    CursorExpired,
    SetupRequired,
    UnsupportedMedia,
    RootOverlap,
    RootUnavailable,
    DeviceUnavailable,
    NotActiveDevice,
    SourceUnavailable,
    SourceTimeout,
    PluginSettingsInvalid,
    Internal,
}

impl Code {
    /// The status the spec pairs with each code.
    pub fn status(self) -> StatusCode {
        match self {
            Self::Unauthenticated | Self::SessionRevoked | Self::InvalidCredentials => {
                StatusCode::UNAUTHORIZED
            }
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::VersionConflict | Self::DeviceUnavailable | Self::NotActiveDevice => {
                StatusCode::CONFLICT
            }
            Self::CursorExpired => StatusCode::GONE,
            Self::UnsupportedMedia => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::ValidationFailed
            | Self::WeakPassword
            | Self::BreachedPassword
            | Self::UsernameTaken
            | Self::RootOverlap
            | Self::RootUnavailable
            | Self::PluginSettingsInvalid => StatusCode::UNPROCESSABLE_ENTITY,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            Self::CapacityExceeded | Self::SetupRequired | Self::SourceUnavailable => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::SourceTimeout => StatusCode::GATEWAY_TIMEOUT,
        }
    }
}

/// An error response. Its status follows from its code.
#[derive(Debug, Serialize)]
pub struct Problem {
    #[serde(rename = "type")]
    kind: &'static str,
    title: &'static str,
    status: u16,
    code: Code,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

impl Problem {
    pub fn new(code: Code) -> Problem {
        let status = code.status();
        Problem {
            // No per-code documents to link to, so RFC 9457's "about:blank", whose title is the
            // status phrase.
            kind: "about:blank",
            title: status.canonical_reason().unwrap_or("Error"),
            status: status.as_u16(),
            code,
            detail: None,
        }
    }

    /// Adds an explanation. It reaches the client, so it must be safe to show.
    pub fn detail(mut self, detail: impl Into<String>) -> Problem {
        self.detail = Some(detail.into());
        self
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = self.code.status();
        let content_type = [(header::CONTENT_TYPE, "application/problem+json")];
        (status, content_type, Json(self)).into_response()
    }
}

impl From<DbError> for Problem {
    fn from(error: DbError) -> Problem {
        // The cause stays in the log: it can describe the schema or the host.
        tracing::error!(%error, "database error");
        Problem::new(Code::Internal)
    }
}
