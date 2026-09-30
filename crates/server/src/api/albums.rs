//! Albums: `listAlbums` and `getAlbum` (`api/openapi.yaml`).

mod representation;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde::Deserialize;

pub use representation::{Album, AlbumSummary, SELECT, summary};

use super::credit::ArtistCredit;
use super::extract::{Path, Query};
use super::page::{Order, Page, Request, Sort, Source, Unknown};
use super::{Id, Problem};
use crate::db::Database;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AlbumSort {
    #[default]
    Name,
    Artist,
    DateAdded,
    ReleaseDate,
    Duration,
    // `playCount` and `lastPlayed` wait for listening history, and answer `422` until then.
}

impl AlbumSort {
    /// Each is one of `albums`' indexes after `library_id`.
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
    artist_id: Option<String>,
    #[serde(default)]
    artist_credit: ArtistCredit,
    cursor: Option<String>,
    limit: Option<u32>,
}

pub async fn list(
    State(db): State<Arc<Database>>,
    Path(Id(library)): Path<Id>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<AlbumSummary>>, Problem> {
    let request = Request::new(query.sort.sort(), query.order, query.cursor, query.limit)?;
    let filtered = query.artist_id.is_some();
    let source = source(library, query.artist_id.as_deref(), query.artist_credit);
    db.read(move |conn| request.read(conn, library, "album_count", filtered, &source, summary))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

pub async fn get(
    State(db): State<Arc<Database>>,
    Path((Id(library), Id(album))): Path<(Id, Id)>,
) -> Result<Json<Album>, Problem> {
    db.read(move |conn| representation::album(conn, library, album))
        .await?
        .map(Json)
        .ok_or_else(Problem::not_found)
}

/// Albums credited to this artist in the album artists.
const OWNED: &str = "SELECT album_id FROM album_artists WHERE artist_id = ?";
/// Albums with a track credited to this artist.
const ON_TRACKS: &str = "SELECT tracks.album_id FROM track_artists \
     JOIN tracks ON tracks.id = track_artists.track_id WHERE track_artists.artist_id = ?";

fn source(library: i64, artist: Option<&str>, credit: ArtistCredit) -> Source {
    let (from, mut params) = match artist {
        None => ("albums al".to_owned(), Vec::new()),
        Some(artist) => {
            let (credited, params) = credit.query(OWNED, ON_TRACKS, artist);
            // An artist's albums are few next to the library's, so the query starts from them
            // and sorts them, rather than walking the whole sort index; `CROSS JOIN` fixes that
            // order. The library filter still applies to every album it reaches.
            let from =
                format!("({credited}) credited CROSS JOIN albums al ON al.id = credited.album_id");
            (from, params)
        }
    };
    params.push(library.into());
    Source {
        select: SELECT,
        from,
        filter: "al.library_id = ?".to_owned(),
        params,
    }
}

#[cfg(test)]
mod tests;
