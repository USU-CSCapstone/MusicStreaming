//! The routes under `/me`: the caller's account, devices, playback, plays, and plugin
//! connections.

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post, put};
use tower_http::compression::CompressionLayer;

use crate::api::{AppState, connections, devices, me, player, plays, queue};

/// Every route under `/me`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me", get(me::get))
        .route("/me/devices", get(devices::list))
        .route("/me/player", get(player::get))
        .route("/me/player/commands", post(player::send_command))
        .route(
            "/me/queue",
            // A queue's state grows with its edits, so the snapshot is sent gzipped to a client
            // that asks. A start carries its tracks, so it takes a larger body than axum's
            // default.
            get(queue::get)
                .layer(CompressionLayer::new())
                .merge(post(queue::start).layer(DefaultBodyLimit::max(queue::BODY_LIMIT))),
        )
        // An add carries its tracks, so an edit takes a body as large as a start does.
        .route("/me/queue/edits", post(queue::edit).layer(DefaultBodyLimit::max(queue::BODY_LIMIT)))
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
