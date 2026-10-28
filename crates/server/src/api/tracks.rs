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
use super::page::{self, Check, Order, Page, Request, Sort, Source, Table, Unknown};
use super::scope::Scope;
use super::tag_filter::{self, TagMatch, Tagged, Tags};
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
    tag: Option<String>,
    #[serde(default)]
    tag_match: TagMatch,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// The library's tracks, or those on `albumId`, credited to `artistId` as `artistCredit` says,
/// and carrying the tags `tag` names as `tagMatch` says, a page at a time.
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
    let tags = tag_filter::ids(query.tag.as_deref());
    let tag_match = query.tag_match;
    let page = db
        .read(move |conn| {
            let source = source(conn, library, album, artist, credit, &tags, tag_match)?;
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

/// How tracks carry tags.
const TAGGED: Tagged = Tagged { links: "track_tags", owner: "track_id", count: "track_count" };

/// The tracks `list` pages through in `library`: every one, or those on `album`, credited to
/// `artist` as `credit` says, and carrying the tags `tags` name as `tag_match` says. An artist's
/// tracks lead a page; failing that, the tags' may ([`Tags::plan`]).
fn source(
    conn: &Connection,
    library: i64,
    album: Option<AnyId>,
    artist: Option<AnyId>,
    credit: ArtistCredit,
    tags: &[AnyId],
    tag_match: TagMatch,
) -> rusqlite::Result<Source> {
    let lead = artist.map(|artist| credit.lead(conn, library, &CREDITS, artist)).transpose()?;
    let tags = Tags::read(conn, library, &TAGGED, tags, tag_match)?;
    let rows = match tags {
        Tags::Groups(_) => page::library_number(conn, library, "track_count")?,
        _ => 0,
    };
    let alone = album.is_none() && artist.is_none();
    let plan = tags.plan(&TAGGED, "t", lead, rows, alone);
    let album = album.map(|album| Check {
        condition: "t.album_id = ?".to_owned(),
        params: vec![album.0.into()],
    });
    let mut source =
        TRACKS.source(conn, library, plan.lead, Check::all(album.into_iter().chain(plan.check)))?;
    if let Some(total) = plan.total {
        source.total = total;
    }
    Ok(source)
}

#[cfg(test)]
mod tests;
