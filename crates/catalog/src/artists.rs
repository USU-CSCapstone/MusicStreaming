//! Artists: the listing and a single artist (`listArtists` and `getArtist`,
//! `api/openapi.yaml`).

mod representation;

use rusqlite::Connection;
use serde::Deserialize;

pub use representation::{Artist, ArtistSummary, SELECT, artist, summary};

use crate::page::{Page, Request, Sort, Source, Table, Unknown};
use crate::tag_filter::{self, LEAD_SHARE, TagFilter, Tagged};

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
/// page `request` asks for.
pub fn list(
    conn: &Connection,
    library: i64,
    request: Request,
    tags: &TagFilter,
) -> rusqlite::Result<Page<ArtistSummary>> {
    let source = source(conn, library, tags)?;
    request.read(conn, &source, summary)
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
