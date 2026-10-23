//! A play report's `context` and `reason`, as the spec's `Context` and `Reason`
//! (`api/openapi.yaml`). A play stores them as JSON written back from these types, so it keeps
//! only what the spec defines: fields it does not name are dropped, and a missing field, a value
//! of the wrong type or a name outside one of its enums makes the report malformed.

use serde::{Deserialize, Serialize};

use crate::api::time::Timestamp;

/// The spec's `Context`: what a play was played from. IDs are kept as given, since the spec
/// calls them opaque.
#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Context {
    Track(Entity),
    Album(Entity),
    Playlist(Entity),
    Tag(Entity),
    Mix(Entity),
    Artist {
        id: String,
        #[serde(default)]
        scope: ArtistScope,
    },
    Search {
        query: String,
        section: Section,
        #[serde(skip_serializing_if = "Option::is_none")]
        within: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filter: Option<BrowseFilter>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sort: Option<Sort>,
    },
    Filter(View),
    Library(View),
    Radio {
        seed: EntityRef,
    },
    External {
        source_id: String,
        item_id: String,
    },
}

/// A track, album, playlist, tag or mix context: the spec's `EntityContext`.
#[derive(Deserialize, Serialize)]
pub struct Entity {
    id: String,
}

/// The spec's `Reason`: why a recommendation or automatic track is there.
#[derive(Deserialize, Serialize)]
pub struct Reason {
    kind: ReasonKind,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    refs: Option<Vec<EntityRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<Timestamp>,
}

/// A reason's `kind`.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReasonKind {
    BecauseYouListened,
    SimilarTo,
    NotHeardSince,
    NeverPlayed,
    Anniversary,
    Collaborator,
    RadioSeed,
    EndlessPlay,
    TimeOfDay,
    Decade,
    Mood,
}

/// The spec's `EntityRef`.
#[derive(Deserialize, Serialize)]
pub struct EntityRef {
    #[serde(rename = "type")]
    kind: EntityType,
    id: String,
}

/// The spec's `EntityType`.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EntityType {
    Track,
    Album,
    Artist,
    Playlist,
    Tag,
    Mix,
    Library,
}

/// An artist context's `scope`, `all` when the report leaves it out.
#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtistScope {
    #[default]
    All,
    Discography,
    Appearances,
    TopTracks,
}

/// A search context's `section`: whether the query matched tracks or lyrics.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Section {
    Tracks,
    Lyrics,
}

/// A `filter` or `library` context: a filtered view, or the whole library.
#[derive(Deserialize, Serialize)]
pub struct View {
    #[serde(skip_serializing_if = "Option::is_none")]
    filter: Option<BrowseFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sort: Option<Sort>,
}

/// A search, filter or library context's `sort`.
#[derive(Deserialize, Serialize)]
pub struct Sort {
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    order: Option<Order>,
}

/// A sort's `order`.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Order {
    Asc,
    Desc,
}

/// The spec's `BrowseFilter`.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    genres: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    genre_match: Option<GenreMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    year_from: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    year_to: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    album_types: Option<Vec<AlbumType>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    availability: Option<Availability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    played: Option<Played>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_played_before: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_played_after: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_play_count: Option<i64>,
}

/// A filter's `genreMatch`: whether items carry all or any of its genres.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GenreMatch {
    All,
    Any,
}

/// The spec's `AlbumType`.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AlbumType {
    Album,
    Ep,
    Single,
    Compilation,
}

/// The spec's `Availability`.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Availability {
    Available,
    Missing,
}

/// A filter's `played`: whether the caller has never or ever played the items.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Played {
    Never,
    Ever,
}

#[cfg(test)]
mod tests;
