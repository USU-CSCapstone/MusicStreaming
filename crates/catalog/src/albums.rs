//! Albums: the listing and a single album (`listAlbums` and `getAlbum`, `api/openapi.yaml`).

mod representation;

use rusqlite::Connection;
use serde::Deserialize;

pub use representation::{Album, AlbumSummary, SELECT, summary};

use crate::credit::{ArtistCredit, Credits};
use crate::page::{Page, Request, Sort, Source, Table, Unknown};
use crate::tag_filter::{self, LEAD_SHARE, TagFilter, Tagged, Tags};
use crate::{AnyId, hidden};

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
/// the tags `tags` names, as the page `request` asks for `user`: without those they hid whole
/// ([`Losses::whole_albums`](hidden::Losses)), and those the filters reach only through tracks
/// they hid, each counting only what they did not hide.
pub fn list(
    conn: &Connection,
    user: i64,
    library: i64,
    request: Request,
    artist: Option<AnyId>,
    credit: ArtistCredit,
    tags: &TagFilter,
) -> rusqlite::Result<Page<AlbumSummary>> {
    let mut source = source(conn, library, artist, credit, tags)?;
    let losses = hidden::losses(conn, user, library)?;
    let mut gone = losses.whole_albums.clone();
    let owners = losses.albums.iter().map(|(&album, loss)| (album, &loss.tags));
    gone.extend(Tags::read(conn, library, &TAGGED, tags)?.lost(conn, &TAGGED, owners)?);
    if let (Some(AnyId(Some(artist))), ArtistCredit::Featured | ArtistCredit::Any) =
        (artist, credit)
        && let Some(loss) = losses.artists.get(&artist)
    {
        gone.extend(&loss.featured_albums);
    }
    hidden::leave_out_ids(conn, &mut source, "al.id", &gone)?;
    let mut page = request.read(conn, &source, summary)?;
    page.items.iter_mut().for_each(|item| item.lose(&losses));
    Ok(page)
}

/// Album `album` of `library` as `user` sees it, or `None` when the library has no such album or
/// they hid it whole.
pub fn album(
    conn: &Connection,
    user: i64,
    library: i64,
    album: i64,
) -> rusqlite::Result<Option<Album>> {
    let losses = hidden::losses(conn, user, library)?;
    if losses.whole_albums.contains(&album) {
        return Ok(None);
    }
    let mut found = representation::album(conn, library, album)?;
    if let Some(album) = &mut found {
        album.lose(&losses);
    }
    Ok(found)
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
