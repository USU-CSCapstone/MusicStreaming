//! The routes under `/libraries`: the libraries the caller reaches and the catalog in each.

use axum::Router;
use axum::routing::{delete, get};

use crate::api::{
    AppState, albums, artists, audio, images, libraries, lyrics, playlists, recent_searches,
    search, tags, tracks, waveform,
};

/// Every route under `/libraries`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/libraries", get(libraries::list))
        .route("/libraries/{library_id}", get(libraries::get))
        .route("/libraries/{library_id}/albums", get(albums::list))
        .route("/libraries/{library_id}/artists", get(artists::list))
        .route("/libraries/{library_id}/playlists", get(playlists::list))
        .route("/libraries/{library_id}/tag-types", get(tags::types))
        .route("/libraries/{library_id}/tags", get(tags::list))
        .route("/libraries/{library_id}/tags/{tag_id}", get(tags::get))
        .route("/libraries/{library_id}/tracks", get(tracks::list))
        .route("/libraries/{library_id}/tracks/{track_id}", get(tracks::get))
        .route("/libraries/{library_id}/search", get(search::search))
        .route(
            "/libraries/{library_id}/recent-searches",
            get(recent_searches::list).post(recent_searches::record).delete(recent_searches::clear),
        )
        .route(
            "/libraries/{library_id}/recent-searches/{recent_search_id}",
            delete(recent_searches::delete),
        )
        .route("/libraries/{library_id}/images/{image_id}", get(images::get))
        .route("/libraries/{library_id}/tracks/{track_id}/lyrics", get(lyrics::get))
        .route("/libraries/{library_id}/tracks/{track_id}/waveform", get(waveform::get))
        .route("/libraries/{library_id}/tracks/{track_id}/playback", get(audio::playback))
        .route("/libraries/{library_id}/tracks/{track_id}/audio", get(audio::audio))
        .route("/libraries/{library_id}/artists/{artist_id}", get(artists::get))
        .route("/libraries/{library_id}/albums/{album_id}", get(albums::get))
}
