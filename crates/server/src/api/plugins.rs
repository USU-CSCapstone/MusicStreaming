//! Plugin administration (`api/openapi.yaml`, Admin): installing, granting permissions,
//! enabling per library, uninstalling, and running (`requirements/plugins.md` §4–5). Admins
//! and the owner only, as everything under `/admin` is (`authenticate`).

mod representation;

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::extract::rejection::{BytesRejection, FailedToBufferBody};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jewelcase_plugins::Permission;
use jewelcase_plugins::manifest::is_host_name;
use jewelcase_plugins::settings::Schema;
use serde::{Deserialize, Serialize};

use super::extract::{self, Path};
use super::{Code, Id, Problem};
use crate::db::Database;
use crate::plugins::{PluginError, Plugins, grants, settings};
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

/// From an uploaded file, or from `{ "url" }` as JSON. Installed disabled everywhere. A file over
/// the route's body limit is `plugin_invalid`; an upload that breaks off or stalls is
/// `validation_failed`.
pub async fn install(
    State(db): State<Arc<Database>>,
    State(plugins): State<Arc<Plugins>>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Problem> {
    let body = body.map_err(|rejection| match rejection {
        // The route's body limit is the size a plugin may be.
        BytesRejection::FailedToBufferBody(FailedToBufferBody::LengthLimitError(_)) => {
            Problem::new(Code::PluginInvalid).detail("The file is over 50 MB.")
        }
        _ => Problem::invalid("The upload did not arrive in full."),
    })?;
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

/// Exempts it from the limit for every plugin on one host (`requirements/plugins.md` §7).
pub async fn exempt(
    State(plugins): State<Arc<Plugins>>,
    Path((id, host)): Path<(String, String)>,
) -> Result<StatusCode, Problem> {
    plugins.add_exemption(id, exempted(&host)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Ends its exemption from the limit for every plugin on one host.
pub async fn unexempt(
    State(plugins): State<Arc<Plugins>>,
    Path((id, host)): Path<(String, String)>,
) -> Result<StatusCode, Problem> {
    plugins.remove_exemption(id, exempted(&host)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `host` as an exemption names it, if it is one host name.
fn exempted(host: &str) -> Result<String, Problem> {
    if !is_host_name(host) {
        return Err(Problem::invalid(
            "An exemption is for one host name, such as musicbrainz.org.",
        ));
    }
    Ok(host.to_ascii_lowercase())
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsLevel {
    library_id: Option<String>,
}

/// The level `query` names: a library that exists, or the server-wide settings.
async fn level(db: &Database, query: SettingsLevel) -> Result<i64, Problem> {
    let Some(library) = query.library_id else { return Ok(settings::SERVER) };
    // A malformed ID names no library, like an unknown one.
    let Some(Id(library)) = Id::canonical(&library) else { return Err(Problem::not_found()) };
    let exists = db
        .read(move |conn| {
            conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM libraries WHERE id = ?1)",
                [library],
                |row| row.get(0),
            )
        })
        .await?;
    if exists { Ok(library) } else { Err(Problem::not_found()) }
}

/// The spec's `PluginSettings`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSettings {
    schema: serde_json::Value,
    values: serde_json::Map<String, serde_json::Value>,
    secrets_set: Vec<String>,
}

impl PluginSettings {
    /// `values` set for `schema`, secrets left out, and which secrets have a value.
    pub fn new(schema: Schema, mut values: serde_json::Map<String, serde_json::Value>) -> Self {
        let secrets_set =
            schema.secrets().filter(|name| values.contains_key(*name)).map(str::to_owned).collect();
        values.retain(|name, _| !schema.secrets().any(|secret| secret == name));
        // As JSON Schema has it, an object of these properties.
        let mut schema = serde_json::to_value(&schema).expect("a schema serializes");
        schema["type"] = "object".into();
        PluginSettings { schema, values, secrets_set }
    }
}

/// What is set at a level, secrets left out, and which secrets have a value.
pub async fn get_settings(
    State(db): State<Arc<Database>>,
    Path(id): Path<String>,
    extract::Query(query): extract::Query<SettingsLevel>,
) -> Result<Json<PluginSettings>, Problem> {
    let level = level(&db, query).await?;
    db.read(move |conn| {
        let Some(manifest) = grants::manifest(conn, &id)? else { return Ok(None) };
        let schema = manifest.settings.unwrap_or_default();
        Ok(Some(PluginSettings::new(schema, settings::level(conn, &id, level)?)))
    })
    .await?
    .map(Json)
    .ok_or_else(Problem::not_found)
}

/// The spec's `PluginSettingsUpdate`.
#[derive(Deserialize)]
pub struct SettingsUpdate {
    pub values: serde_json::Map<String, serde_json::Value>,
}

pub async fn set_settings(
    State(db): State<Arc<Database>>,
    State(plugins): State<Arc<Plugins>>,
    Path(id): Path<String>,
    extract::Query(query): extract::Query<SettingsLevel>,
    extract::Json(update): extract::Json<SettingsUpdate>,
) -> Result<Json<PluginSettings>, Problem> {
    let level_id = query.library_id.clone();
    let level = level(&db, query).await?;
    plugins.save_settings(id.clone(), level, update.values).await?;
    get_settings(State(db), Path(id), extract::Query(SettingsLevel { library_id: level_id })).await
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
            PluginError::SettingsInvalid(detail) => {
                Problem::new(Code::PluginSettingsInvalid).detail(detail)
            }
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
