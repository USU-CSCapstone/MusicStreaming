//! Albums: `listAlbums` and `getAlbum` (`api/openapi.yaml`).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::credit::ArtistCredit;
use super::page::{self, Order, Page, Sort, Source, Unknown};
use super::query::Query;
use super::refs::{self, Credit, ImageRef, TagRef};
use super::sql::timestamp;
use super::{Code, Id, Problem};
use crate::db::Database;

/// The spec's `AlbumSummary`, without `personal` until accounts exist.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummary {
    id: Id,
    title: Option<String>,
    artists: Vec<Credit>,
    release_date: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    genres: Vec<TagRef>,
    track_count: i64,
    track_total: Option<i64>,
    disc_count: i64,
    disc_total: Option<i64>,
    duration_us: i64,
    image: Option<ImageRef>,
    availability: &'static str,
    added_at: String,
}

/// The spec's `Album`.
#[derive(Serialize)]
pub struct Album {
    #[serde(flatten)]
    summary: AlbumSummary,
    labels: Vec<String>,
    discs: Vec<Disc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Disc {
    /// `None` for tracks with no disc tag, which the scanner files as disc 0.
    number: Option<i64>,
    track_count: i64,
    track_total: Option<i64>,
}

/// The columns [`summary`] reads, in order.
pub const SELECT: &[&str] = &[
    "al.id",
    "al.title",
    "al.release_date",
    "al.type",
    "al.track_count",
    // The spec's trackTotal sums the tagged totals of the discs present; NULL if none is tagged.
    "(SELECT sum(track_total) FROM album_discs WHERE album_discs.album_id = al.id)",
    "al.disc_count",
    "al.disc_total",
    "al.duration_us",
    "al.image_id",
    "al.available_track_count > 0",
    timestamp!("al.added_at"),
];

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
    Path(library_id): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<AlbumSummary>>, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    let sort = query.sort.sort();
    let after = sort.after(query.cursor.as_deref(), query.order)?;
    let limit = page::limit(query.limit)?;
    let filtered = query.artist_id.is_some();
    let source = source(library, query.artist_id.as_deref(), query.artist_credit);
    db.read(move |conn| {
        let Some(album_count) = page::library_count(conn, library, "album_count")? else {
            return Ok(None);
        };
        // The scanner keeps the library's count, so only a filtered list counts its rows.
        let total = if filtered {
            page::count(conn, &source)?
        } else {
            album_count
        };
        let (items, next_cursor) =
            page::fetch(conn, &source, sort, query.order, after, limit, |row| {
                summary(conn, row)
            })?;
        Ok(Some(Page {
            items,
            next_cursor,
            total,
        }))
    })
    .await?
    .map(Json)
    .ok_or_else(|| Problem::new(Code::NotFound))
}

pub async fn get(
    State(db): State<Arc<Database>>,
    Path((library_id, album_id)): Path<(String, String)>,
) -> Result<Json<Album>, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    let Id(album) = Id::parse(&album_id)?;
    db.read(move |conn| {
        let sql = format!(
            "SELECT {}, al.labels FROM albums al WHERE al.library_id = ?1 AND al.id = ?2",
            SELECT.join(", ")
        );
        let Some((summary, labels)) = conn
            .prepare_cached(&sql)?
            .query_row([library, album], |row| {
                Ok((summary(conn, row)?, row.get::<_, String>(SELECT.len())?))
            })
            .optional()?
        else {
            return Ok(None);
        };
        let discs = conn
            .prepare_cached(
                "SELECT disc_number, track_count, track_total FROM album_discs \
                 WHERE album_id = ?1 ORDER BY disc_number",
            )?
            .query_map([album], |row| {
                Ok(Disc {
                    number: Some(row.get(0)?).filter(|number| *number != 0),
                    track_count: row.get(1)?,
                    track_total: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(Album {
            summary,
            // The schema checks that labels is a JSON array; the scanner writes strings.
            labels: serde_json::from_str(&labels).unwrap_or_default(),
            discs,
        }))
    })
    .await?
    .map(Json)
    .ok_or_else(|| Problem::new(Code::NotFound))
}

/// Albums credited to this artist in the album artists.
const OWNED: &str = "SELECT album_id FROM album_artists WHERE artist_id = ?";
/// Albums with a track credited to this artist.
const ON_TRACKS: &str = "SELECT tracks.album_id FROM track_artists \
     JOIN tracks ON tracks.id = track_artists.track_id WHERE track_artists.artist_id = ?";

fn source(library: i64, artist: Option<&str>, credit: ArtistCredit) -> Source {
    let Some(artist) = artist else {
        return Source {
            select: SELECT,
            from: "albums al".to_owned(),
            filter: "al.library_id = ?".to_owned(),
            params: vec![library.into()],
        };
    };
    let (credited, mut params) = credit.query(OWNED, ON_TRACKS, artist);
    params.push(library.into());
    Source {
        select: SELECT,
        // An artist's albums are few next to the library's, so the query starts from them and
        // sorts them, rather than walking the whole sort index; `CROSS JOIN` fixes that order.
        // The library filter still applies to every album it reaches.
        from: format!("({credited}) credited CROSS JOIN albums al ON al.id = credited.album_id"),
        filter: "al.library_id = ?".to_owned(),
        params,
    }
}

/// An album from a row of [`SELECT`], with its artists and genres.
pub fn summary(conn: &Connection, row: &Row) -> rusqlite::Result<AlbumSummary> {
    let id: i64 = row.get(0)?;
    Ok(AlbumSummary {
        id: Id(id),
        title: row.get(1)?,
        artists: refs::artists(conn, "album", id)?,
        release_date: row.get(2)?,
        kind: row.get(3)?,
        genres: refs::genres(conn, "album", id)?,
        track_count: row.get(4)?,
        track_total: row.get(5)?,
        disc_count: row.get(6)?,
        disc_total: row.get(7)?,
        duration_us: row.get(8)?,
        image: ImageRef::new(row.get(9)?),
        availability: if row.get(10)? { "available" } else { "missing" },
        added_at: row.get(11)?,
    })
}

#[cfg(test)]
mod tests;
