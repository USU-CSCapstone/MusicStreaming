//! The routes under `/me`: the caller's account, devices, playback, plays, and plugin
//! connections.

use axum::Router;
use axum::routing::{get, post, put};

use crate::api::{AppState, connections, devices, me, player, plays};

/// Every route under `/me`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me", get(me::get))
        .route("/me/devices", get(devices::list))
        .route("/me/player", get(player::get))
        .route("/me/player/commands", post(player::send_command))
        .route("/me/plays", post(plays::record))
        .route("/me/plugins", get(connections::list))
        .route(
            "/me/plugins/{plugin_id}/settings",
            get(connections::get_settings)
                .put(connections::set_settings)
                .delete(connections::disconnect),
        )
        .route("/me/plugins/{plugin_id}/search-sharing", put(connections::set_search_sharing))
}
