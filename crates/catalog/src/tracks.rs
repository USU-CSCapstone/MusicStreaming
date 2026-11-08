//! Tracks: the listing, a single track, and the sorts and filters behind them
//! (`listTracks` and `getTrack`, `api/openapi.yaml`).

mod representation;

use rusqlite::Connection;
use serde::Deserialize;

pub use representation::{
    JOINS, LOUDNESS, Loudness, SELECT, Track, TrackSummary, list_summary, loudness, sort_keys,
    track,
};

use crate::AnyId;
use crate::credit::{ArtistCredit, Credits};
use crate::page::{self, Check, Order, Page, Request, Sort, Source, Table, Unknown};
use crate::tag_filter::{self, LEAD_SHARE, RESOLVE_SHARE, TagFilter, Tagged};

/// A track with no number sorts last on its disc. This matches the indexes' expression
/// (`0003_catalog.sql`), which keeps NULL out of the keyset.
pub const TRACK_NUMBER: &str = "ifnull(t.track_number, 9223372036854775807)";

/// Album order within one album: disc, then number. `tracks_by_album_order` holds exactly this
/// after `album_id`, where the library-wide album order would sort the album on every page.
pub const WITHIN_ALBUM: Sort = Sort {
    label: "tracks.album.withinAlbum",
    columns: &["t.disc_number", TRACK_NUMBER, "t.id"],
    unknown: Unknown::Never,
};

/// The `sort` values `listTracks` supports, and so a recipe.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackSort {
    #[default]
    Name,
    Artist,
    Album,
    DateAdded,
    ReleaseDate,
    Duration,
    // The spec's `playCount`, `lastPlayed`, and `top` are not implemented, so `listTracks` refuses them.
}

impl TrackSort {
    /// The name `listTracks` and a recipe spell this sort by.
    pub fn name(self) -> &'static str {
        match self {
            TrackSort::Name => "name",
            TrackSort::Artist => "artist",
            TrackSort::Album => "album",
            TrackSort::DateAdded => "dateAdded",
            TrackSort::ReleaseDate => "releaseDate",
            TrackSort::Duration => "duration",
        }
    }

    /// The keyset this sort pages by. Each one's columns are one of `tracks`' indexes after
    /// `library_id`.
    pub fn sort(self) -> &'static Sort {
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

/// A track list's sort and direction, as `listTracks` takes them and a recipe keeps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackOrder {
    pub sort: TrackSort,
    pub order: Order,
}

impl TrackOrder {
    /// The sort's and the direction's names, as `listTracks` and a recipe spell them.
    pub fn names(self) -> (&'static str, &'static str) {
        let order = if self.order == Order::Asc { "asc" } else { "desc" };
        (self.sort.name(), order)
    }
}

impl TrackOrder {
    /// The keyset and direction to page by. An album's own track listing, the common case,
    /// reads straight from its index.
    pub fn keyset(self, one_album: bool) -> (&'static Sort, Order) {
        match (self.sort, one_album) {
            (TrackSort::Album, true) => (&WITHIN_ALBUM, self.order),
            (sort, _) => (sort.sort(), self.order),
        }
    }
}

/// The library's tracks, or those on `album`, credited to `artist` as `credit` says, and
/// carrying the tags `tags` names, as the page `request` asks for.
pub fn list(
    conn: &Connection,
    library: i64,
    request: Request,
    album: Option<AnyId>,
    artist: Option<AnyId>,
    credit: ArtistCredit,
    tags: &TagFilter,
) -> rusqlite::Result<Page<TrackSummary>> {
    let source = source(conn, library, album, artist, credit, tags, LEAD_SHARE)?;
    request.read(conn, &source, list_summary)
}

/// The tracks `ids` names that are in `library`, in the order named, as one page with no next
/// cursor. An ID that is unknown or in another library is left out, so the answer never says
/// which; one named twice appears twice.
pub fn by_ids(
    conn: &Connection,
    library: i64,
    ids: &[i64],
) -> rusqlite::Result<Page<TrackSummary>> {
    let items = crate::by_ids::by_ids(conn, SELECT, "tracks t", JOINS, library, ids, list_summary)?;
    let total = i64::try_from(items.len()).unwrap_or(i64::MAX);
    Ok(Page { items, next_cursor: None, total })
}

/// The ID of every track in `library` that `list` pages through for the same `album`, `artist`,
/// `credit`, `tags` and `order`, in the same order, read at once: one statement for each part of
/// the sort's index, not a page at a time. A missing track stays in place, as it does in a view.
/// Being read whole, it starts from a larger tag's tracks than a page does
/// ([`RESOLVE_SHARE`]).
pub fn resolve(
    conn: &Connection,
    library: i64,
    album: Option<AnyId>,
    artist: Option<AnyId>,
    credit: ArtistCredit,
    tags: &TagFilter,
    order: TrackOrder,
) -> rusqlite::Result<Vec<u64>> {
    let (sort, order) = order.keyset(album.is_some());
    let source = source(conn, library, album, artist, credit, tags, RESOLVE_SHARE)?;
    // Only the IDs are wanted, so the columns and joins for summaries are left out.
    let source = Source { select: &["t.id"], joins: "", ..source };
    page::ids(conn, &source, sort, order)
}

/// The tracks a list reads.
pub const TRACKS: Table =
    Table { name: "tracks", alias: "t", select: SELECT, joins: JOINS, count: "track_count" };

/// How tracks are credited to an artist.
pub const CREDITS: Credits = Credits {
    owned: "SELECT tracks.id FROM album_artists \
         JOIN tracks ON tracks.album_id = album_artists.album_id WHERE album_artists.artist_id = ?",
    on_tracks: "SELECT track_id AS id FROM track_artists WHERE artist_id = ?",
    owned_count: OWNED_TRACKS,
    featured_count: FEATURED_TRACKS,
};

/// An expression over `artists` (aliased `ar`) counting the tracks on the albums that list the
/// artist, counted from the tracks themselves: a device has no `album_groups`, and the albums'
/// stored counts are the groups' sums only where the scanner kept them.
pub const OWNED_TRACKS: &str = "(SELECT count(*) FROM album_artists aa \
     JOIN tracks ot ON ot.album_id = aa.album_id WHERE aa.artist_id = ar.id)";

/// An expression over `artists` (aliased `ar`) counting the tracks that credit the artist on
/// albums that do not list it.
pub const FEATURED_TRACKS: &str = "(SELECT ifnull(sum(ac.track_count), 0) FROM album_credits ac \
     WHERE ac.artist_id = ar.id AND NOT EXISTS (SELECT 1 FROM album_artists aa \
         WHERE aa.album_id = ac.album_id AND aa.artist_id = ac.artist_id))";

/// How tracks carry tags.
pub const TAGGED: Tagged = Tagged { links: "track_tags", owner: "track_id", count: "track_count" };

/// The tracks `list` pages through in `library`: every one, or those on `album`, credited to
/// `artist` as `credit` says, and carrying the tags `tags` names. An artist's
/// tracks lead a page; failing that, the tags' may, by `share` ([`tag_filter::source`]).
pub fn source(
    conn: &Connection,
    library: i64,
    album: Option<AnyId>,
    artist: Option<AnyId>,
    credit: ArtistCredit,
    tags: &TagFilter,
    share: i64,
) -> rusqlite::Result<Source> {
    let lead = artist.map(|artist| credit.lead(conn, library, &CREDITS, artist)).transpose()?;
    let album = album.map(|album| Check {
        condition: "t.album_id = ?".to_owned(),
        params: vec![album.0.into()],
    });
    tag_filter::source(conn, library, &TRACKS, &TAGGED, lead, album, tags, share)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recipe_reads_each_sorts_ids_from_its_index_alone() {
        let conn = Connection::open_in_memory().unwrap();
        crate::create_schema(&conn).unwrap();
        for sort in [
            TrackSort::Name,
            TrackSort::Artist,
            TrackSort::Album,
            TrackSort::DateAdded,
            TrackSort::ReleaseDate,
            TrackSort::Duration,
        ] {
            let columns = sort.sort().columns.join(", ");
            let plan: Vec<String> = conn
                .prepare(&format!(
                    "EXPLAIN QUERY PLAN SELECT t.id FROM tracks t \
                     WHERE t.library_id = 1 ORDER BY {columns}"
                ))
                .unwrap()
                .query_map([], |row| row.get(3))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            let plan = plan.join("; ");
            assert!(plan.contains("COVERING INDEX tracks_by_"), "{}: {plan}", sort.name());
            assert!(!plan.contains("TEMP B-TREE"), "{}: {plan}", sort.name());
        }
    }
}
