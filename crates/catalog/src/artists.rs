//! Artists: the listing and a single artist (`listArtists` and `getArtist`,
//! `api/openapi.yaml`).

mod representation;

use rusqlite::Connection;
use serde::Deserialize;

pub use representation::{Artist, ArtistSummary, SELECT, summary};

use crate::hidden;
use crate::page::{Page, Request, Sort, Source, Table, Unknown};
use crate::tag_filter::{self, LEAD_SHARE, TagFilter, Tagged, Tags};

/// The `sort` values `listArtists` supports.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtistSort {
    #[default]
    Name,
    DateAdded,
    // The spec's `playCount` and `lastPlayed` are not implemented, so they answer `422`.
}

impl ArtistSort {
    /// The keyset this sort pages by. Each one's columns are one of `artists`' indexes after
    /// `library_id`.
    pub fn sort(self) -> &'static Sort {
        match self {
            ArtistSort::Name => &Sort {
                label: "artists.name",
                columns: &["ar.sort_key", "ar.id"],
                unknown: Unknown::Empty,
            },
            ArtistSort::DateAdded => &Sort {
                label: "artists.dateAdded",
                columns: &["ar.added_at", "ar.id"],
                unknown: Unknown::Never,
            },
        }
    }
}

/// Every artist credited in the library, featured-only ones included
/// (`requirements/artists.md` §5), or those whose work carries the tags `tags` names, as the
/// page `request` asks for `user`: without those they hid every track of, and those the tags
/// reach only through tracks they hid, each counting only what they did not hide.
pub fn list(
    conn: &Connection,
    user: i64,
    library: i64,
    request: Request,
    tags: &TagFilter,
) -> rusqlite::Result<Page<ArtistSummary>> {
    let mut source = source(conn, library, tags)?;
    let losses = hidden::losses(conn, user, library)?;
    let mut gone = losses.whole_artists.clone();
    let owners = losses.artists.iter().map(|(&artist, loss)| (artist, &loss.tags));
    gone.extend(Tags::read(conn, library, &TAGGED, tags)?.lost(conn, &TAGGED, owners)?);
    hidden::leave_out_ids(conn, &mut source, "ar.id", &gone)?;
    let mut page = request.read(conn, &source, summary)?;
    page.items.iter_mut().for_each(|item| item.lose(&losses));
    Ok(page)
}

/// Artist `artist` of `library` as `user` sees them, or `None` when the library has no such
/// artist or they hid every track of theirs.
pub fn artist(
    conn: &Connection,
    user: i64,
    library: i64,
    artist: i64,
) -> rusqlite::Result<Option<Artist>> {
    let losses = hidden::losses(conn, user, library)?;
    if losses.whole_artists.contains(&artist) {
        return Ok(None);
    }
    let mut found = representation::artist(conn, library, artist)?;
    if let Some(artist) = &mut found {
        artist.lose(&losses);
    }
    Ok(found)
}

/// The artists a list reads.
const ARTISTS: Table =
    Table { name: "artists", alias: "ar", select: SELECT, joins: "", count: "artist_count" };

/// How artists carry tags: those of the tracks that credit them (`requirements/tags.md` §6).
pub const TAGGED: Tagged =
    Tagged { links: "artist_tags", owner: "artist_id", count: "artist_count" };

/// The artists `list` pages through in `library`: every one, or those carrying the tags `tags`
/// names.
pub fn source(conn: &Connection, library: i64, tags: &TagFilter) -> rusqlite::Result<Source> {
    tag_filter::source(conn, library, &ARTISTS, &TAGGED, None, None, tags, LEAD_SHARE)
}
