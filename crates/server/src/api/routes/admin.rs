//! The routes under `/admin`, which only admins and the owner pass
//! (`authenticate::require_admin`).

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post, put};

use crate::api::{AppState, plugins};

/// Every route under `/admin`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/admin/plugins",
            // A plugin file may be larger than the default limit on a request body.
            get(plugins::list)
                .post(plugins::install)
                .layer(DefaultBodyLimit::max(jewelcase_plugins::MAX_SIZE)),
        )
        .route("/admin/plugins/{plugin_id}", get(plugins::get).delete(plugins::uninstall))
        .route("/admin/plugins/{plugin_id}/permissions", put(plugins::set_permissions))
        .route(
            "/admin/plugins/{plugin_id}/exemptions/{host}",
            put(plugins::exempt).delete(plugins::unexempt),
        )
        .route("/admin/plugins/{plugin_id}/libraries/{library_id}", put(plugins::set_enabled))
        .route("/admin/plugins/{plugin_id}/run", post(plugins::run))
        .route(
            "/admin/plugins/{plugin_id}/settings",
            get(plugins::get_settings).put(plugins::set_settings),
        )
}
