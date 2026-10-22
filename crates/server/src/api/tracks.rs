//! Tracks: `listTracks` and `getTrack` (`api/openapi.yaml`).

mod representation;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde::Deserialize;

pub use representation::{
    JOINS, LOUDNESS, Loudness, SELECT, Track, TrackSummary, list_summary, loudness,
};

use super::credit::ArtistCredit;
use super::extract::{Path, Query};
use super::page::{Order, Page, Request, Sort, Source, Unknown};
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
    // `playCount`, `lastPlayed`, and `top` wait for listening history, and answer `422` until
    // then.
}

impl TrackSort {
    /// Each is one of `tracks`' indexes after `library_id`.
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
    Path(Id(library)): Path<Id>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<TrackSummary>>, Problem> {
    // An album's own track listing, the common case, reads straight from its index.
    let sort = match (query.sort, &query.album_id) {
        (TrackSort::Album, Some(_)) => &WITHIN_ALBUM,
        (sort, _) => sort.sort(),
    };
    let request = Request::new(sort, query.order, query.cursor, query.limit)?;
    let filtered = query.album_id.is_some() || query.artist_id.is_some();
    let source = source(library, query.album_id, query.artist_id, query.artist_credit);
    let page = db
        .read(move |conn| {
            request.read(conn, library, "track_count", filtered, &source, list_summary)
        })
        .await?;
    Ok(Json(page))
}

pub async fn get(
    State(db): State<Arc<Database>>,
    Path((Id(library), Id(track))): Path<(Id, Id)>,
) -> Result<Json<Track>, Problem> {
    db.read(move |conn| representation::track(conn, library, track))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

/// Tracks whose album this artist is an album artist of.
const OWNED: &str = "SELECT tracks.id AS track_id FROM album_artists \
     JOIN tracks ON tracks.album_id = album_artists.album_id WHERE album_artists.artist_id = ?";
/// Tracks credited to this artist.
const ON_TRACKS: &str = "SELECT track_id FROM track_artists WHERE artist_id = ?";

/// The tracks `list` pages through in `library`: every one, or those on `album` and credited
/// to `artist` as `credit` says.
fn source(
    library: i64,
    album: Option<AnyId>,
    artist: Option<AnyId>,
    credit: ArtistCredit,
) -> Source {
    let (from, mut params) = match artist {
        None => ("tracks t".to_owned(), Vec::new()),
        Some(artist) => {
            let (credited, params) = credit.query(OWNED, ON_TRACKS, artist);
            // An artist's tracks are few next to the library's, so the query starts from them
            // and sorts them; `CROSS JOIN` fixes that order.
            let from =
                format!("({credited}) credited CROSS JOIN tracks t ON t.id = credited.track_id");
            (from, params)
        }
    };
    let mut filter = "t.library_id = ?".to_owned();
    params.push(library.into());
    if let Some(album) = album {
        filter += " AND t.album_id = ?";
        params.push(album.0.into());
    }
    Source { select: SELECT, from, joins: JOINS, filter, params }
}

#[cfg(test)]
mod tests;
