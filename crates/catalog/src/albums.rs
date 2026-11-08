//! Albums: the listing and a single album (`listAlbums` and `getAlbum`, `api/openapi.yaml`).

mod representation;

use rusqlite::Connection;
use serde::Deserialize;

pub use representation::{Album, AlbumSummary, SELECT, album, summary};

use crate::AnyId;
use crate::credit::{ArtistCredit, Credits};
use crate::page::{Page, Request, Sort, Source, Table, Unknown};
use crate::tag_filter::{self, LEAD_SHARE, TagFilter, Tagged};

/// The `sort` values `listAlbums` supports.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AlbumSort {
    #[default]
    Name,
    Artist,
    DateAdded,
    ReleaseDate,
    Duration,
    // The spec's `playCount` and `lastPlayed` are not implemented, so they answer `422`.
}

impl AlbumSort {
    /// The keyset this sort pages by. Each one's columns are one of `albums`' indexes after
    /// `library_id`.
    pub fn sort(self) -> &'static Sort {
        match self {
            AlbumSort::Name => &Sort {
                label: "albums.name",
                columns: &["al.sort_key", "al.id"],
                unknown: Unknown::Empty,
            },
            AlbumSort::Artist => &Sort {
                label: "albums.artist",
                columns: &["al.artist_sort_key", "al.sort_key", "al.id"],
                unknown: Unknown::Empty,
            },
            AlbumSort::DateAdded => &Sort {
                label: "albums.dateAdded",
                columns: &["al.added_at", "al.id"],
                unknown: Unknown::Never,
            },
            AlbumSort::ReleaseDate => &Sort {
                label: "albums.releaseDate",
                columns: &["al.release_date", "al.sort_key", "al.id"],
                unknown: Unknown::Null,
            },
            AlbumSort::Duration => &Sort {
                label: "albums.duration",
                columns: &["al.duration_us", "al.id"],
                unknown: Unknown::Never,
            },
        }
    }
}

/// The library's albums, or those credited to `artist` as `credit` says and whose tracks carry
/// the tags `tags` names, as the page `request` asks for.
pub fn list(
    conn: &Connection,
    library: i64,
    request: Request,
    artist: Option<AnyId>,
    credit: ArtistCredit,
    tags: &TagFilter,
) -> rusqlite::Result<Page<AlbumSummary>> {
    let source = source(conn, library, artist, credit, tags)?;
    request.read(conn, &source, summary)
}

/// The albums a list reads.
const ALBUMS: Table =
    Table { name: "albums", alias: "al", select: SELECT, joins: "", count: "album_count" };

/// An expression over `artists` (aliased `ar`) counting the albums a track credits the artist on
/// but whose album artists do not list it.
const FEATURED_ALBUMS: &str = "(SELECT count(*) FROM album_credits ac WHERE ac.artist_id = ar.id \
     AND NOT EXISTS (SELECT 1 FROM album_artists aa \
                     WHERE aa.album_id = ac.album_id AND aa.artist_id = ac.artist_id))";

/// How albums are credited to an artist.
const CREDITS: Credits = Credits {
    owned: "SELECT album_id AS id FROM album_artists WHERE artist_id = ?",
    on_tracks: "SELECT album_id AS id FROM album_credits WHERE artist_id = ?",
    owned_count: "album_count",
    featured_count: FEATURED_ALBUMS,
};

/// How albums carry tags: those of their tracks (`requirements/tags.md` §6).
pub const TAGGED: Tagged = Tagged { links: "album_tags", owner: "album_id", count: "album_count" };

/// The albums `list` pages through in `library`: every one, or those credited to `artist` as
/// `credit` says and carrying the tags `tags` names.
pub fn source(
    conn: &Connection,
    library: i64,
    artist: Option<AnyId>,
    credit: ArtistCredit,
    tags: &TagFilter,
) -> rusqlite::Result<Source> {
    let lead = artist.map(|artist| credit.lead(conn, library, &CREDITS, artist)).transpose()?;
    tag_filter::source(conn, library, &ALBUMS, &TAGGED, lead, None, tags, LEAD_SHARE)
}
