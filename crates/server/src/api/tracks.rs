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
use crate::db::{self, Database};

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

/// The most `ids` one request may name: a default page.
const MAX_IDS: usize = super::page::DEFAULT_LIMIT as usize;

/// The parameters `listTracks` supports so far. Any other, including the spec's other
/// filters, answers `422` rather than being ignored. The ones with defaults are optional so
/// that `ids`, which excludes every other parameter, can tell they were not sent.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    sort: Option<TrackSort>,
    order: Option<Order>,
    album_id: Option<AnyId>,
    artist_id: Option<AnyId>,
    artist_credit: Option<ArtistCredit>,
    cursor: Option<String>,
    limit: Option<u32>,
    /// Comma-separated track IDs, as the spec's `ids` says.
    ids: Option<String>,
}

impl ListQuery {
    /// Whether any parameter but `ids` was sent.
    fn has_other_parameters(&self) -> bool {
        self.sort.is_some()
            || self.order.is_some()
            || self.album_id.is_some()
            || self.artist_id.is_some()
            || self.artist_credit.is_some()
            || self.cursor.is_some()
            || self.limit.is_some()
    }
}

/// The tracks `ids` names that are in `library`, in the order named, as one page. An ID that
/// is malformed, unknown, or in another library is left out, so the answer never says which.
/// More than [`MAX_IDS`] IDs, or any parameter besides `ids`, is `422`.
async fn by_ids(
    db: Arc<Database>,
    library: i64,
    query: ListQuery,
) -> Result<Json<Page<TrackSummary>>, Problem> {
    if query.has_other_parameters() {
        return Err(Problem::invalid("ids cannot be combined with other parameters"));
    }
    let text = query.ids.unwrap_or_default();
    let ids: Vec<i64> = text.split(',').filter_map(|id| AnyId::from(id).0).collect();
    if text.split(',').count() > MAX_IDS {
        return Err(Problem::invalid(format!("ids names at most {MAX_IDS} tracks")));
    }
    let items = db
        .read(move |conn| db::by_ids(conn, SELECT, "tracks t", JOINS, library, &ids, list_summary))
        .await?;
    let total = i64::try_from(items.len()).unwrap_or(i64::MAX);
    Ok(Json(Page { items, next_cursor: None, total }))
}

/// The library's tracks, or those on `albumId` and credited to `artistId` as `artistCredit`
/// says, a page at a time; or, given `ids`, just those tracks (see [`by_ids`]).
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<TrackSummary>>, Problem> {
    if query.ids.is_some() {
        return by_ids(db, library, query).await;
    }
    // An album's own track listing, the common case, reads straight from its index.
    let sort = match (query.sort.unwrap_or_default(), &query.album_id) {
        (TrackSort::Album, Some(_)) => &WITHIN_ALBUM,
        (sort, _) => sort.sort(),
    };
    let order = query.order.unwrap_or_default();
    let request = Request::new(sort, order, query.cursor, query.limit)?;
    let credit = query.artist_credit.unwrap_or_default();
    let (album, artist) = (query.album_id, query.artist_id);
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
    owned_count: OWNED_TRACKS,
    featured_count: FEATURED_TRACKS,
};

/// An expression over `artists` (aliased `ar`) counting the tracks on the albums that list the
/// artist.
const OWNED_TRACKS: &str = "(SELECT ifnull(sum(g.track_count), 0) FROM album_artists aa \
     JOIN album_groups g ON g.album_id = aa.album_id WHERE aa.artist_id = ar.id)";

/// An expression over `artists` (aliased `ar`) counting the tracks that credit the artist on
/// albums that do not list it.
pub const FEATURED_TRACKS: &str = "(SELECT ifnull(sum(ac.track_count), 0) FROM album_credits ac \
     WHERE ac.artist_id = ar.id AND NOT EXISTS (SELECT 1 FROM album_artists aa \
         WHERE aa.album_id = ac.album_id AND aa.artist_id = ac.artist_id))";

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
    let check = album.map(|album| Check {
        condition: "t.album_id = ?".to_owned(),
        params: vec![album.0.into()],
    });
    TRACKS.source(conn, library, lead, check)
}

#[cfg(test)]
mod tests;
