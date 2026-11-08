//! Errors as RFC 9457 problem details: the spec's `Problem` (`api/openapi.yaml`).
//!
//! Every error the API returns is one of these. Handlers return `Result<T, Problem>`, and `?`
//! turns a database error into a `500 internal` whose cause is logged, never sent.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use super::realtime::Revoked;
use crate::db::DbError;

/// The spec's `Problem.code`: what went wrong, for clients to act on. Only the codes some
/// endpoint returns; the spec lists more.
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
    PluginInvalid,
    PluginSettingsInvalid,
    PluginExists,
    QueueChanged,
    QueueActive,
    AnchorGone,
    PlayGone,
    DuplicateKey,
    PermissionsRequired,
    VersionConflict,
    CursorExpired,
    CatalogBuilding,
    DeviceUnavailable,
    SetupRequired,
    Internal,
}

impl Code {
    /// The status the spec pairs with each code.
    pub fn status(self) -> StatusCode {
        match self {
            Self::Unauthenticated | Self::SessionRevoked | Self::InvalidCredentials => {
                StatusCode::UNAUTHORIZED
            }
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::ValidationFailed
            | Self::DuplicateKey
            | Self::WeakPassword
            | Self::PluginInvalid
            | Self::PluginSettingsInvalid => StatusCode::UNPROCESSABLE_ENTITY,
            Self::PluginExists
            | Self::PermissionsRequired
            | Self::VersionConflict
            | Self::DeviceUnavailable
            | Self::QueueChanged
            | Self::QueueActive
            | Self::AnchorGone
            | Self::PlayGone => StatusCode::CONFLICT,
            Self::SetupRequired | Self::CatalogBuilding => StatusCode::SERVICE_UNAVAILABLE,
            Self::CursorExpired => StatusCode::GONE,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
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
    /// Why the device was signed out, with `session_revoked`.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<Revoked>,
    /// The version the resource is at now, with `version_conflict`.
    #[serde(rename = "currentVersion", skip_serializing_if = "Option::is_none")]
    current_version: Option<i64>,
    /// When to try again, in whole seconds, with `rate_limited` or `catalog_building`: sent as `Retry-After`.
    #[serde(skip)]
    retry_after: Option<i64>,
}

impl Problem {
    /// A problem with `code`, its status, and no explanation.
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
            reason: None,
            current_version: None,
            retry_after: None,
        }
    }

    /// A `404 not_found`, which is also what anything out of the caller's reach answers.
    pub fn not_found() -> Problem {
        Problem::new(Code::NotFound)
    }

    /// A `422 validation_failed` with an explanation, which must be safe to show.
    pub fn invalid(detail: impl Into<String>) -> Problem {
        Problem::new(Code::ValidationFailed).detail(detail)
    }

    /// Adds an explanation. It reaches the client, so it must be safe to show.
    pub fn detail(mut self, detail: impl Into<String>) -> Problem {
        self.detail = Some(detail.into());
        self
    }

    /// A `401 session_revoked`: the device was signed out, for `reason`.
    pub fn revoked(reason: Revoked) -> Problem {
        Problem { reason: Some(reason), ..Problem::new(Code::SessionRevoked) }
    }

    /// A `409 version_conflict`: the edit was made on a version, and the resource is at
    /// `current` now.
    pub fn version_conflict(current: i64) -> Problem {
        Problem { current_version: Some(current), ..Problem::new(Code::VersionConflict) }
            .detail("The playlist changed since this edit was made; nothing was applied.")
    }

    /// A `429 rate_limited` that says, in whole seconds rounded up, when to try again.
    pub fn rate_limited(wait_ms: i64) -> Problem {
        let seconds = (wait_ms + 999) / 1000;
        Problem { retry_after: Some(seconds), ..Problem::new(Code::RateLimited) }
    }

    /// A `503 catalog_building` that says, in whole seconds, when to try again.
    pub fn catalog_building(retry_after: i64) -> Problem {
        Problem { retry_after: Some(retry_after), ..Problem::new(Code::CatalogBuilding) }
    }

    /// Its explanation, if it has one.
    pub fn explanation(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl IntoResponse for Problem {
    /// The problem as `application/problem+json` with its status, and with `Retry-After` when
    /// it says when to try again.
    fn into_response(self) -> Response {
        let status = self.code.status();
        let content_type = [(header::CONTENT_TYPE, "application/problem+json")];
        let retry_after =
            self.retry_after.map(|seconds| [(header::RETRY_AFTER, HeaderValue::from(seconds))]);
        (status, content_type, retry_after, Json(self)).into_response()
    }
}

impl From<DbError> for Problem {
    /// A `500 internal`, with the error logged.
    fn from(error: DbError) -> Problem {
        // The cause stays in the log: it can describe the schema or the host.
        tracing::error!(%error, "database error");
        Problem::new(Code::Internal)
    }
}

impl From<jewelcase_catalog::Invalid> for Problem {
    /// A `422 validation_failed` with the catalog's explanation.
    fn from(invalid: jewelcase_catalog::Invalid) -> Problem {
        Problem::invalid(invalid.0)
    }
}

impl From<jewelcase_catalog::playlists::ItemsError> for Problem {
    /// An invalid parameter is `422`, a playlist that is not the caller's `404`, and a cursor
    /// from another version `410`. An item without its track breaks retention: `500`, logged.
    fn from(error: jewelcase_catalog::playlists::ItemsError) -> Problem {
        use jewelcase_catalog::playlists::ItemsError;
        match error {
            ItemsError::Invalid(invalid) => invalid.into(),
            ItemsError::NotFound => Problem::not_found(),
            ItemsError::Expired => Problem::new(Code::CursorExpired)
                .detail("The playlist changed since the first page; start again."),
            ItemsError::Incomplete => {
                tracing::error!("a playlist item names a track that is gone");
                Problem::new(Code::Internal)
            }
            ItemsError::Sql(error) => error.into(),
        }
    }
}

impl From<rusqlite::Error> for Problem {
    /// A database error, logged and answered as a `500 internal`, as a [`DbError`] is.
    fn from(error: rusqlite::Error) -> Problem {
        DbError::from(error).into()
    }
}
