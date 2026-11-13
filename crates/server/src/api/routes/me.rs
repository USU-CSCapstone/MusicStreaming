//! The routes under `/me`: the caller's account, devices, playback, plays, and plugin
//! connections.

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use tower_http::compression::CompressionLayer;

use crate::api::{AppState, account_changes, connections, devices, me, player, plays, queue};

/// Every route under `/me`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me", get(me::get))
        // Rows repeat the same few names, so a page is gzipped like the library's feed.
        .route("/me/changes", get(account_changes::list).layer(CompressionLayer::new()))
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
        // A list is gzipped like the snapshot, and no other answer is.
        .route("/me/queue/lists/{hash}", get(queue::get_list).layer(CompressionLayer::new()))
        .route("/me/queue/rows", get(queue::get_rows))
        .route("/me/queues", get(queue::list))
        // Not gzipped: the idempotency layer stores the answer as the handler made it, and a
        // compressed body has no size it can know.
        .route("/me/queues/{queue_id}/restore", post(queue::restore))
        // A kept queue's state is gzipped like the snapshot; a GET is never stored as an answer.
        .route(
            "/me/queues/{queue_id}",
            get(queue::get_kept).layer(CompressionLayer::new()).merge(delete(queue::delete)),
        )
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
