//! A user's own plugins (`api/openapi.yaml`, Account): those that act for each person, such as
//! a scrobbler, which they connect with their own account on its service
//! (`requirements/plugins.md` §4.1, §6). Only plugins enabled in a library the caller reaches
//! are offered; any other answers `404`, as one that does not exist.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use super::Problem;
use super::authenticate::Caller;
use super::extract::{self, Path};
use super::plugins::{PluginSettings, SettingsUpdate};
use crate::db::Database;
use crate::plugins::Plugins;
use crate::plugins::personal::{self, SharingRefused};

/// The spec's `PersonalPlugin`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalPlugin {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    connected: bool,
    /// Whether the caller could share their searches with it.
    asks_for_searches: bool,
    shares_searches: bool,
}

#[derive(Serialize)]
pub struct PersonalPluginList {
    items: Vec<PersonalPlugin>,
}

pub async fn list(
    State(db): State<Arc<Database>>,
    caller: Caller,
) -> Result<Json<PersonalPluginList>, Problem> {
    let found = db.read(move |conn| personal::connectable(conn, caller.user, caller.admin)).await?;
    let items = found
        .into_iter()
        .map(|p| PersonalPlugin {
            id: p.id,
            name: p.name,
            description: p.description,
            connected: p.connected,
            asks_for_searches: p.asks_for_searches,
            shares_searches: p.shares_searches,
        })
        .collect();
    Ok(Json(PersonalPluginList { items }))
}

/// What the caller has set, secrets left out: nothing until they connect it.
pub async fn get_settings(
    State(db): State<Arc<Database>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Json<PluginSettings>, Problem> {
    db.read(move |conn| {
        let Some(manifest) = personal::manifest(conn, &id, caller.user, caller.admin)? else {
            return Ok(None);
        };
        let schema = manifest.personal_settings.unwrap_or_default();
        Ok(Some(PluginSettings::new(schema, personal::values(conn, &id, caller.user)?)))
    })
    .await?
    .map(Json)
    .ok_or_else(Problem::not_found)
}

/// Connects the plugin, or changes what it was connected with, once it accepts them.
pub async fn set_settings(
    State(db): State<Arc<Database>>,
    State(plugins): State<Arc<Plugins>>,
    caller: Caller,
    Path(id): Path<String>,
    extract::Json(update): extract::Json<SettingsUpdate>,
) -> Result<Json<PluginSettings>, Problem> {
    plugins.save_personal(id.clone(), caller.user, caller.admin, update.values).await?;
    get_settings(State(db), caller, Path(id)).await
}

/// The spec's `SearchSharing`.
#[derive(Deserialize)]
pub struct SearchSharing {
    sharing: bool,
}

/// Turns sharing the caller's searches with it on or off (`requirements/search.md` §6).
pub async fn set_search_sharing(
    State(db): State<Arc<Database>>,
    caller: Caller,
    Path(id): Path<String>,
    extract::Json(SearchSharing { sharing }): extract::Json<SearchSharing>,
) -> Result<StatusCode, Problem> {
    let set = db
        .write(move |tx| personal::set_search_sharing(tx, &id, caller.user, caller.admin, sharing))
        .await?;
    match set {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(SharingRefused::Unknown) => Err(Problem::not_found()),
        Err(SharingRefused::Unconnected) => {
            Err(Problem::invalid("Connect it before sharing your searches with it."))
        }
        Err(SharingRefused::SearchesNotAllowed) => {
            Err(Problem::invalid("This plugin has not been allowed your searches."))
        }
    }
}

/// Disconnects it: what the caller set is forgotten, and it stops acting for them.
pub async fn disconnect(
    State(db): State<Arc<Database>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    db.write(move |tx| personal::disconnect(tx, &id, caller.user)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
