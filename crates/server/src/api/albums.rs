//! Albums: `listAlbums` and `getAlbum` (`api/openapi.yaml`).

mod representation;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::Connection;
use serde::Deserialize;

pub use representation::{Album, AlbumSummary, SELECT, summary};

use super::credit::{ArtistCredit, Credits};
use super::extract::{Path, Query};
use super::page::{Order, Page, Request, Sort, Source, Table, Unknown};
use super::scope::Scope;
use super::{AnyId, Id, Problem};
use crate::db::Database;

/// The `sort` values `listAlbums` supports.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AlbumSort {
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
    fn sort(self) -> &'static Sort {
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

/// The parameters `listAlbums` supports so far. Any other, including the spec's other
/// filters, answers `422` rather than being ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    sort: AlbumSort,
    #[serde(default)]
    order: Order,
    artist_id: Option<AnyId>,
    #[serde(default)]
    artist_credit: ArtistCredit,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// The library's albums, or those credited to `artistId` as `artistCredit` says, a page at a
/// time.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<AlbumSummary>>, Problem> {
    let request = Request::new(query.sort.sort(), query.order, query.cursor, query.limit)?;
    let (artist, credit) = (query.artist_id, query.artist_credit);
    let page = db
        .read(move |conn| {
            let source = source(conn, library, artist, credit)?;
            request.read(conn, &source, summary)
        })
        .await?;
    Ok(Json(page))
}

/// `getAlbum`: the album, or not found when the library has no such album.
pub async fn get(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Path((_, Id(album))): Path<(Id, Id)>,
) -> Result<Json<Album>, Problem> {
    db.read(move |conn| representation::album(conn, library, album))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

/// The albums a list reads.
const ALBUMS: Table =
    Table { name: "albums", alias: "al", select: SELECT, joins: "", count: "album_count" };

/// How albums are credited to an artist.
const CREDITS: Credits = Credits {
    owned: "SELECT album_id AS id FROM album_artists WHERE artist_id = ?",
    on_tracks: "SELECT album_id AS id FROM album_credits WHERE artist_id = ?",
    owned_count: "album_count",
    featured_count: "any_album_count - album_count",
    any_count: "any_album_count",
};

/// The albums `list` pages through in `library`: every one, or those credited to `artist` as
/// `credit` says.
fn source(
    conn: &Connection,
    library: i64,
    artist: Option<AnyId>,
    credit: ArtistCredit,
) -> rusqlite::Result<Source> {
    let lead = artist.map(|artist| credit.lead(conn, library, &CREDITS, artist)).transpose()?;
    ALBUMS.source(conn, library, lead, Vec::new())
}

#[cfg(test)]
mod tests;
