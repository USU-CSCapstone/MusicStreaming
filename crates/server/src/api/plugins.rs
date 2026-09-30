//! Plugin administration (`api/openapi.yaml`, Admin): installing, granting permissions,
//! enabling per library, uninstalling, and running (`requirements/plugins.md` §4–5). Admins
//! and the owner only, as everything under `/admin` is (`authenticate`).

mod representation;

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::extract::rejection::BytesRejection;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jewelcase_plugins::Permission;
use serde::{Deserialize, Serialize};

use super::extract::{self, Path};
use super::{Code, Id, Problem};
use crate::db::Database;
use crate::plugins::{PluginError, Plugins};
use representation::Plugin;

#[derive(Serialize)]
pub struct PluginList {
    items: Vec<Plugin>,
}

pub async fn list(State(db): State<Arc<Database>>) -> Result<Json<PluginList>, Problem> {
    Ok(Json(PluginList { items: db.read(representation::all).await? }))
}

pub async fn get(
    State(db): State<Arc<Database>>,
    Path(id): Path<String>,
) -> Result<Json<Plugin>, Problem> {
    Ok(Json(plugin(&db, id).await?))
}

#[derive(Deserialize)]
struct FromUrl {
    url: String,
}

/// From an uploaded file, or from `{ "url" }` as JSON. Installed disabled everywhere.
pub async fn install(
    State(db): State<Arc<Database>>,
    State(plugins): State<Arc<Plugins>>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Problem> {
    // The only way to exceed the route's body limit is a file over the size a plugin may be.
    let body =
        body.map_err(|_| Problem::new(Code::PluginInvalid).detail("The file is over 50 MB."))?;
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    let id = if json {
        let FromUrl { url } = serde_json::from_slice(&body)
            .map_err(|_| Problem::invalid("Send the plugin's URL as { \"url\": \"…\" }."))?;
        plugins.install_url(url).await?
    } else {
        plugins.install(body.to_vec(), None).await?
    };
    Ok((StatusCode::CREATED, Json(plugin(&db, id).await?)).into_response())
}

pub async fn uninstall(
    State(plugins): State<Arc<Plugins>>,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    plugins.uninstall(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The body of `PUT …/permissions`. Names it does not know, or that the plugin never asked
/// for, are ignored rather than refused, as a grant can never add what was not requested.
#[derive(Deserialize)]
pub struct PermissionGrants {
    granted: Vec<String>,
    libraries: Vec<LibraryGrants>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryGrants {
    library_id: String,
    granted: Vec<String>,
}

fn permissions(names: &[String]) -> Vec<Permission> {
    names.iter().filter_map(|name| Permission::from_name(name)).collect()
}

pub async fn set_permissions(
    State(db): State<Arc<Database>>,
    State(plugins): State<Arc<Plugins>>,
    Path(id): Path<String>,
    extract::Json(grants): extract::Json<PermissionGrants>,
) -> Result<Json<Plugin>, Problem> {
    let per_library = grants
        .libraries
        .iter()
        // A malformed ID names no library, like an unknown one.
        .filter_map(|l| Some((Id::canonical(&l.library_id)?.0, permissions(&l.granted))))
        .collect();
    plugins.set_grants(id.clone(), permissions(&grants.granted), per_library).await?;
    Ok(Json(plugin(&db, id).await?))
}

#[derive(Deserialize)]
pub struct Enabled {
    enabled: bool,
}

/// The library must exist: `authenticate` answers `404` for one that does not.
pub async fn set_enabled(
    State(db): State<Arc<Database>>,
    State(plugins): State<Arc<Plugins>>,
    Path((id, Id(library))): Path<(String, Id)>,
    extract::Json(Enabled { enabled }): extract::Json<Enabled>,
) -> Result<Json<Plugin>, Problem> {
    plugins.set_enabled(id.clone(), library, enabled).await?;
    Ok(Json(plugin(&db, id).await?))
}

/// The spec's `PluginRunResult`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    ok: bool,
    summary: String,
    log: Vec<String>,
    /// Files it saved into the library.
    saved: usize,
    /// Whether the scanner will pick those up: a scan of their folders is queued.
    scanner_running: bool,
}

/// Runs the plugin once in each library it is enabled in, and answers when it is done.
pub async fn run(
    State(plugins): State<Arc<Plugins>>,
    Path(id): Path<String>,
) -> Result<Json<RunResult>, Problem> {
    let result = plugins.run(id).await?;
    Ok(Json(RunResult {
        ok: result.ok,
        summary: result.summary,
        log: result.log,
        saved: result.saved,
        scanner_running: result.scanning,
    }))
}

async fn plugin(db: &Database, id: String) -> Result<Plugin, Problem> {
    db.read(move |conn| representation::get(conn, &id)).await?.ok_or_else(Problem::not_found)
}

impl From<PluginError> for Problem {
    fn from(error: PluginError) -> Problem {
        match error {
            PluginError::NotFound => Problem::not_found(),
            PluginError::Invalid(detail) => Problem::new(Code::PluginInvalid).detail(detail),
            PluginError::Exists(_) => Problem::new(Code::PluginExists).detail(error.to_string()),
            PluginError::PermissionsRequired(_) => {
                Problem::new(Code::PermissionsRequired).detail(error.to_string())
            }
            PluginError::Db(error) => Problem::from(error),
            PluginError::Internal(detail) => Problem::new(Code::Internal).detail(detail),
        }
    }
}

#[cfg(test)]
mod tests;
