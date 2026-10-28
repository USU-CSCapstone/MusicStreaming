//! Tags: `listTagTypes`, `listTags` and `getTag` (`api/openapi.yaml`).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::extract::{Path, Query};
use super::page::{Order, Page, Request, Sort, Source, Total, Unknown};
use super::scope::Scope;
use super::{Id, Problem};
use crate::db::Database;

/// The spec's `TagType`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagType {
    key: String,
    tag_count: i64,
}

/// The spec's `Tag`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    id: Id,
    r#type: String,
    name: String,
    track_count: i64,
    album_count: i64,
    artist_count: i64,
}

/// The columns [`tag`] reads, in order.
const SELECT: &[&str] =
    &["g.id", "g.type", "g.name", "g.track_count", "g.album_count", "g.artist_count"];

/// A tag from a row of [`SELECT`].
fn tag(row: &Row) -> rusqlite::Result<Tag> {
    Ok(Tag {
        id: Id(row.get(0)?),
        r#type: row.get(1)?,
        name: row.get(2)?,
        track_count: row.get(3)?,
        album_count: row.get(4)?,
        artist_count: row.get(5)?,
    })
}

/// The `sort` values `listTags` supports.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
enum TagSort {
    #[default]
    Name,
    TrackCount,
}

impl TagSort {
    /// The keyset this sort pages by. Each one's columns follow `library_id` and `type` in one
    /// of `tags`' indexes, so a list of one type walks it; a list of every type sorts the
    /// library's tags, of which there are few beside its tracks. Every tag has a sort key by the
    /// time its batch commits.
    fn sort(self) -> &'static Sort {
        match self {
            TagSort::Name => &Sort {
                label: "tags.name",
                columns: &["g.sort_key", "g.id"],
                unknown: Unknown::Never,
            },
            TagSort::TrackCount => &Sort {
                label: "tags.trackCount",
                columns: &["g.track_count", "g.id"],
                unknown: Unknown::Never,
            },
        }
    }
}

/// The parameters of `listTags`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    r#type: Option<String>,
    #[serde(default)]
    sort: TagSort,
    #[serde(default)]
    order: Order,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// `listTagTypes`: each type the library has a tag of, with its count of tags, in key order. A
/// tag goes with the last track that carries it, so every type listed has music behind it.
pub async fn types(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
) -> Result<Json<Vec<TagType>>, Problem> {
    let types = db
        .read(move |conn| {
            conn.prepare_cached(
                "SELECT type, count(*) FROM tags WHERE library_id = ?1 GROUP BY type ORDER BY type",
            )?
            .query_map([library], |row| Ok(TagType { key: row.get(0)?, tag_count: row.get(1)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;
    Ok(Json(types))
}

/// `listTags`: the library's tags, of `type` when it names one. A type the library has no tag
/// of lists none.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<Tag>>, Problem> {
    let request = Request::new(query.sort.sort(), query.order, query.cursor, query.limit)?;
    let source = source(library, query.r#type);
    let page = db.read(move |conn| request.read(conn, &source, tag)).await?;
    Ok(Json(page))
}

/// `getTag`: the tag, or not found when the library has no such tag.
pub async fn get(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Path((_, Id(id))): Path<(Id, Id)>,
) -> Result<Json<Tag>, Problem> {
    db.read(move |conn| find(conn, library, id)).await?.map(Json).ok_or_else(Problem::not_found)
}

/// The tags of `library`, of `tag_type` when there is one.
fn source(library: i64, tag_type: Option<String>) -> Source {
    let mut source = Source {
        select: SELECT,
        from: "tags g".to_owned(),
        joins: "",
        filter: "g.library_id = ?".to_owned(),
        params: vec![library.into()],
        total: Total::Counted,
    };
    if let Some(tag_type) = tag_type {
        source.filter += " AND g.type = ?";
        source.params.push(tag_type.into());
    }
    source
}

/// Tag `id` of `library`, if it has one.
fn find(conn: &Connection, library: i64, id: i64) -> rusqlite::Result<Option<Tag>> {
    let sql =
        format!("SELECT {} FROM tags g WHERE g.library_id = ?1 AND g.id = ?2", SELECT.join(", "));
    conn.prepare_cached(&sql)?.query_row([library, id], tag).optional()
}

#[cfg(test)]
mod tests;
