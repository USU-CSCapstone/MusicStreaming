//! Artists: `listArtists` and `getArtist` (`api/openapi.yaml`).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::page::{self, Order, Page, Sort, Source, Unknown, timestamp};
use super::query::Query;
use super::refs::{ImageRef, TagRef};
use super::{Code, Id, Problem};
use crate::db::Database;

/// The spec's `ArtistSummary`, without `personal` until accounts exist.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistSummary {
    id: Id,
    /// `None` for the unknown artist.
    name: Option<String>,
    image: Option<ImageRef>,
    /// Albums the artist owns.
    album_count: i64,
    /// Owned and featured tracks.
    track_count: i64,
    added_at: String,
}

/// The spec's `Artist`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    #[serde(flatten)]
    summary: ArtistSummary,
    biography: Option<String>,
    genres: Vec<TagRef>,
    appearance_count: i64,
}

/// The columns [`summary`] reads, in order.
pub const SELECT: &[&str] = &[
    "ar.id",
    "ar.name",
    "ar.image_id",
    "ar.album_count",
    "ar.track_count",
    timestamp!("ar.added_at"),
];

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ArtistSort {
    #[default]
    Name,
    DateAdded,
    // `playCount` and `lastPlayed` wait for listening history, and answer `422` until then.
}

impl ArtistSort {
    /// Each is one of `artists`' indexes after `library_id`.
    fn sort(self) -> &'static Sort {
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

/// The parameters `listArtists` supports so far. Any other, including the spec's filters,
/// answers `422` rather than being ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    sort: ArtistSort,
    #[serde(default)]
    order: Order,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// Every artist credited in the library, featured-only ones included
/// (`requirements/artists.md` §5).
pub async fn list(
    State(db): State<Arc<Database>>,
    Path(library_id): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<ArtistSummary>>, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    let sort = query.sort.sort();
    let after = sort.after(query.cursor.as_deref(), query.order)?;
    let limit = page::limit(query.limit)?;
    let source = source(library);
    db.read(move |conn| {
        let Some(total) = page::library_count(conn, library, "artist_count")? else {
            return Ok(None);
        };
        let (items, next_cursor) =
            page::fetch(conn, &source, sort, query.order, after, limit, summary)?;
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
    Path((library_id, artist_id)): Path<(String, String)>,
) -> Result<Json<Artist>, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    let Id(artist) = Id::parse(&artist_id)?;
    db.read(move |conn| {
        let sql = format!(
            "SELECT {}, ar.biography, ar.appearance_count FROM artists ar \
             WHERE ar.library_id = ?1 AND ar.id = ?2",
            SELECT.join(", ")
        );
        let Some((summary, biography, appearance_count)) = conn
            .prepare_cached(&sql)?
            .query_row([library, artist], |row| {
                Ok((
                    summary(row)?,
                    row.get(SELECT.len())?,
                    row.get(SELECT.len() + 1)?,
                ))
            })
            .optional()?
        else {
            return Ok(None);
        };
        Ok(Some(Artist {
            summary,
            biography,
            genres: genres(conn, artist)?,
            appearance_count,
        }))
    })
    .await?
    .map(Json)
    .ok_or_else(|| Problem::new(Code::NotFound))
}

fn source(library: i64) -> Source {
    Source {
        select: SELECT,
        from: "artists ar".to_owned(),
        filter: "ar.library_id = ?".to_owned(),
        params: vec![library.into()],
    }
}

/// An artist from a row of [`SELECT`].
pub fn summary(row: &Row) -> rusqlite::Result<ArtistSummary> {
    Ok(ArtistSummary {
        id: Id(row.get(0)?),
        name: row.get(1)?,
        image: ImageRef::new(row.get(2)?),
        album_count: row.get(3)?,
        track_count: row.get(4)?,
        added_at: row.get(5)?,
    })
}

fn genres(conn: &Connection, artist: i64) -> rusqlite::Result<Vec<TagRef>> {
    conn.prepare_cached(
        "SELECT tags.id, tags.name FROM artist_tags JOIN tags ON tags.id = artist_tags.tag_id \
         WHERE artist_tags.artist_id = ?1 ORDER BY tags.sort_key, tags.id",
    )?
    .query_map([artist], |row| {
        Ok(TagRef {
            id: Id(row.get(0)?),
            name: row.get(1)?,
        })
    })?
    .collect()
}

#[cfg(test)]
mod tests;
