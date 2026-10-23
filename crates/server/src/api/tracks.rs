//! Tracks: `listTracks` and `getTrack` (`api/openapi.yaml`).

mod representation;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::Connection;
use serde::Deserialize;

pub use representation::{
    JOINS, LOUDNESS, Loudness, SELECT, Track, TrackSummary, list_summary, loudness,
};

use super::credit::{ArtistCredit, Credits};
use super::extract::{Path, Query};
use super::page::{Check, Order, Page, Request, Sort, Source, Table, Unknown};
use super::scope::Scope;
use super::{AnyId, Id, Problem};
use crate::db::Database;

/// A track with no number sorts last on its disc. This matches the indexes' expression
/// (`0003_catalog.sql`), which keeps NULL out of the keyset.
const TRACK_NUMBER: &str = "ifnull(t.track_number, 9223372036854775807)";

/// Album order within one album: disc, then number. `tracks_by_album_order` holds exactly this
/// after `album_id`, where the library-wide album order would sort the album on every page.
const WITHIN_ALBUM: Sort = Sort {
    label: "tracks.album.withinAlbum",
    columns: &["t.disc_number", TRACK_NUMBER, "t.id"],
    unknown: Unknown::Never,
};

/// The `sort` values `listTracks` supports.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum TrackSort {
    #[default]
    Name,
    Artist,
    Album,
    DateAdded,
    ReleaseDate,
    Duration,
    // The spec's `playCount`, `lastPlayed`, and `top` are not implemented, so they answer `422`.
}

impl TrackSort {
    /// The keyset this sort pages by. Each one's columns are one of `tracks`' indexes after
    /// `library_id`.
    fn sort(self) -> &'static Sort {
        match self {
            TrackSort::Name => &Sort {
                label: "tracks.name",
                columns: &["t.sort_key", "t.id"],
                unknown: Unknown::Empty,
            },
            TrackSort::Artist => &Sort {
                label: "tracks.artist",
                columns: &["t.artist_sort_key", "t.sort_key", "t.id"],
                unknown: Unknown::Empty,
            },
            TrackSort::Album => &Sort {
                label: "tracks.album",
                columns: &["t.album_sort_key", "t.album_id", "t.disc_number", TRACK_NUMBER, "t.id"],
                unknown: Unknown::Empty,
            },
            TrackSort::DateAdded => &Sort {
                label: "tracks.dateAdded",
                columns: &["t.added_at", "t.id"],
                unknown: Unknown::Never,
            },
            TrackSort::ReleaseDate => &Sort {
                label: "tracks.releaseDate",
                columns: &[
                    "t.release_date",
                    "t.album_sort_key",
                    "t.album_id",
                    "t.disc_number",
                    TRACK_NUMBER,
                    "t.id",
                ],
                unknown: Unknown::Null,
            },
            TrackSort::Duration => &Sort {
                label: "tracks.duration",
                columns: &["t.duration_us", "t.id"],
                unknown: Unknown::Never,
            },
        }
    }
}

/// The parameters `listTracks` supports so far. Any other, including the spec's other
/// filters, answers `422` rather than being ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    sort: TrackSort,
    #[serde(default)]
    order: Order,
    album_id: Option<AnyId>,
    artist_id: Option<AnyId>,
    #[serde(default)]
    artist_credit: ArtistCredit,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// The library's tracks, or those on `albumId` and credited to `artistId` as `artistCredit`
/// says, a page at a time.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<TrackSummary>>, Problem> {
    // An album's own track listing, the common case, reads straight from its index.
    let sort = match (query.sort, &query.album_id) {
        (TrackSort::Album, Some(_)) => &WITHIN_ALBUM,
        (sort, _) => sort.sort(),
    };
    let request = Request::new(sort, query.order, query.cursor, query.limit)?;
    let (album, artist, credit) = (query.album_id, query.artist_id, query.artist_credit);
    let page = db
        .read(move |conn| {
            let source = source(conn, library, album, artist, credit)?;
            request.read(conn, &source, list_summary)
        })
        .await?;
    Ok(Json(page))
}

/// `getTrack`: the track, or not found when the library has no such track.
pub async fn get(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
) -> Result<Json<Track>, Problem> {
    db.read(move |conn| representation::track(conn, library, track))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

/// The tracks a list reads.
const TRACKS: Table =
    Table { name: "tracks", alias: "t", select: SELECT, joins: JOINS, count: "track_count" };

/// How tracks are credited to an artist.
const CREDITS: Credits = Credits {
    owned: "SELECT tracks.id FROM album_artists \
         JOIN tracks ON tracks.album_id = album_artists.album_id WHERE album_artists.artist_id = ?",
    on_tracks: "SELECT track_id AS id FROM track_artists WHERE artist_id = ?",
    owned_count: "owned_track_count",
    featured_count: "appearance_count",
    any_count: "owned_track_count + appearance_count",
};

/// The tracks `list` pages through in `library`: every one, or those on `album` and credited
/// to `artist` as `credit` says.
fn source(
    conn: &Connection,
    library: i64,
    album: Option<AnyId>,
    artist: Option<AnyId>,
    credit: ArtistCredit,
) -> rusqlite::Result<Source> {
    let lead = artist.map(|artist| credit.lead(conn, library, &CREDITS, artist)).transpose()?;
    let checks = album.map(|album| Check {
        condition: "t.album_id = ?".to_owned(),
        params: vec![album.0.into()],
    });
    TRACKS.source(conn, library, lead, checks.into_iter().collect())
}

#[cfg(test)]
mod tests;
