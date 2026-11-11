//! Tracks: the listing, a single track, and the sorts and filters behind them
//! (`listTracks` and `getTrack`, `api/openapi.yaml`).

mod representation;

use std::collections::HashSet;

use rusqlite::Connection;
use serde::Deserialize;

pub use representation::{
    JOINS, LOUDNESS, Loudness, SELECT, Track, TrackSummary, list_summary, loudness, mark_hidden,
    mark_liked, sort_keys,
};

use crate::credit::{ArtistCredit, Credits};
use crate::page::{self, Check, Order, Page, Request, Sort, Source, Table, Total, Unknown};
use crate::tag_filter::{self, LEAD_SHARE, RESOLVE_SHARE, TagFilter, Tagged, Tags};
use crate::{AnyId, hidden};

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
    // The spec's `playCount`, `lastPlayed`, and `top` are not implemented, so `listTracks` refuses
    // them.
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
/// carrying the tags `tags` names, as the page `request` asks for `user`: without the tracks
/// they hid, and each marked if they like it.
#[expect(clippy::too_many_arguments, reason = "a list's own parameters, and whose it is")]
pub fn list(
    conn: &Connection,
    user: i64,
    library: i64,
    request: Request,
    album: Option<AnyId>,
    artist: Option<AnyId>,
    credit: ArtistCredit,
    tags: &TagFilter,
) -> rusqlite::Result<Page<TrackSummary>> {
    let mut source = source(conn, library, album, artist, credit, tags, LEAD_SHARE)?;
    hidden::leave_out(conn, &mut source, user, library)?;
    let mut page = request.read(conn, &source, list_summary)?;
    mark_liked(conn, user, library, &mut page.items)?;
    Ok(page)
}

/// The tracks `ids` names that are in `library`, in the order named, as one page with no next
/// cursor, each marked if `user` likes it or hid it: a lookup by ID returns hidden tracks too,
/// so a device holding one it queued learns to skip it. An ID that is unknown or in another
/// library is left out, so the answer never says which; one named twice appears twice.
pub fn by_ids(
    conn: &Connection,
    user: i64,
    library: i64,
    ids: &[i64],
) -> rusqlite::Result<Page<TrackSummary>> {
    let mut items =
        crate::by_ids::by_ids(conn, SELECT, "tracks t", JOINS, library, ids, list_summary)?;
    mark_liked(conn, user, library, &mut items)?;
    mark_hidden(conn, user, library, &mut items)?;
    let total = i64::try_from(items.len()).unwrap_or(i64::MAX);
    Ok(Page { items, next_cursor: None, total })
}

/// Track `track` of `library`, marked if `user` likes it or hid it, or `None` if the library has
/// no such track.
pub fn track(
    conn: &Connection,
    user: i64,
    library: i64,
    track: i64,
) -> rusqlite::Result<Option<Track>> {
    let mut found = representation::track(conn, library, track)?;
    if let Some(track) = &mut found {
        track.mark(conn, user, library)?;
    }
    Ok(found)
}

/// The ID of every track in `library` that `list` pages through for `user` with the same
/// `album`, `artist`, `credit`, `tags` and `order`, in the same order, so without those `user`
/// hid, read at once: one statement for each part of the sort's index, not a page at a time. A
/// missing track stays in place, as it does in a view. Being read whole, it starts from a larger
/// tag's tracks than a page does ([`RESOLVE_SHARE`]), and where every tag is larger, it keeps the
/// tracks of the sort's index whose IDs are in the tags' set.
#[expect(clippy::too_many_arguments, reason = "a list's own parameters, and whose it is")]
pub fn resolve(
    conn: &Connection,
    user: i64,
    library: i64,
    album: Option<AnyId>,
    artist: Option<AnyId>,
    credit: ArtistCredit,
    tags: &TagFilter,
    order: TrackOrder,
) -> rusqlite::Result<Vec<u64>> {
    let (sort, order) = order.keyset(album.is_some());
    let (source, tagged) = resolving(conn, user, library, album, artist, credit, tags)?;
    let mut ids = page::ids(conn, &source, sort, order)?;
    if let Some(tagged) = tagged {
        ids.retain(|id| tagged.contains(&id.cast_signed()));
    }
    Ok(ids)
}

/// What [`resolve`] reads: the tracks' IDs and the sort values in the order of the sort's index,
/// and, where the tags are all too many to start from, the set of IDs those tags make. Such a
/// walk reads each row's ID from the index alone, and the caller keeps those in the set, in
/// place of one lookup of the links for each row.
pub(crate) fn resolving(
    conn: &Connection,
    user: i64,
    library: i64,
    album: Option<AnyId>,
    artist: Option<AnyId>,
    credit: ArtistCredit,
    tags: &TagFilter,
) -> rusqlite::Result<(Source, Option<HashSet<i64>>)> {
    let mut source = source(conn, library, album, artist, credit, tags, RESOLVE_SHARE)?;
    let tagged = match source.walk.take() {
        Some(walk) => {
            (source.filter, source.params) = (walk.filter, walk.params);
            Tags::read(conn, library, &TAGGED, tags)?.ids(conn, &TAGGED)?
        }
        None => None,
    };
    // Only the IDs are wanted, so the columns and joins for summaries are left out, and with them
    // the total, which a stored count would cost a query to correct for what the user hid.
    let mut source = Source {
        select: &["t.id"],
        joins: "",
        total: Total::Counted,
        counting: None,
        back: None,
        ..source
    };
    hidden::leave_out(conn, &mut source, user, library)?;
    Ok((source, tagged))
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
