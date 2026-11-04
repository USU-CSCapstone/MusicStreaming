//! Tags: `listTagTypes`, `listTags` and `getTag` (`api/openapi.yaml`).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_core::tag_sort_key;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::extract::{Path, Query};
use super::hidden::{self, Losses};
use super::page::{Order, Page, Request, Sort, Source, Total, Unknown};
use super::scope::Scope;
use super::{Id, Problem};
use crate::db::Database;

/// The spec's `TagType`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagType {
    key: String,
    /// The name the plugin that added it gave it; none for a built-in type.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
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

impl Tag {
    /// The tag as the user who hid what `losses` counts sees it: its counts of what is left
    /// (`requirements/conventions.md` §8).
    fn lose(&mut self, losses: &Losses) {
        let Some(loss) = losses.tags.get(&self.id.0) else { return };
        self.track_count -= loss.tracks;
        self.album_count -= loss.albums;
        self.artist_count -= loss.artists;
    }
}

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
    q: Option<String>,
    #[serde(default)]
    sort: TagSort,
    #[serde(default)]
    order: Order,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// `listTagTypes`: each type the library has a tag of, with its count of tags and the name a
/// plugin's type is shown by, in key order. A tag goes with the last track that carries it, so
/// every type listed has music behind it. Tags the caller hid whole are not counted, and a type
/// left with none is not listed.
pub async fn types(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
) -> Result<Json<Vec<TagType>>, Problem> {
    let types = db
        .read(move |conn| {
            let whole =
                serde_json::to_string(&hidden::tags(conn, user, library)?).expect("IDs serialize");
            conn.prepare_cached(
                "SELECT used.type, kind.name, used.tags \
                 FROM (SELECT type, count(*) AS tags FROM tags WHERE library_id = ?1 \
                       AND id NOT IN (SELECT value FROM json_each(?2)) GROUP BY type) \
                 used JOIN tag_types kind ON kind.key = used.type ORDER BY used.type",
            )?
            .query_map(rusqlite::params![library, whole], |row| {
                Ok(TagType { key: row.get(0)?, name: row.get(1)?, tag_count: row.get(2)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;
    Ok(Json(types))
}

/// `listTags`: the library's tags, of `type` when it names one and with `q` in their names when
/// it is given, leaving out those the caller hid whole. A type the library has no tag of lists
/// none.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<Tag>>, Problem> {
    let request = Request::new(query.sort.sort(), query.order, query.cursor, query.limit)?;
    let mut source = source(library, query.r#type, query.q.as_deref());
    let page = db
        .read(move |conn| {
            hidden::leave_out_ids(conn, &mut source, "g.id", &hidden::tags(conn, user, library)?)?;
            let mut page = request.read(conn, &source, tag)?;
            let losses = hidden::losses(conn, user, library)?;
            page.items.iter_mut().for_each(|item| item.lose(&losses));
            Ok(page)
        })
        .await?;
    Ok(Json(page))
}

/// `getTag`: the tag, or not found when the library has no such tag, or the caller hid every
/// track carrying it.
pub async fn get(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
    Path((_, Id(id))): Path<(Id, Id)>,
) -> Result<Json<Tag>, Problem> {
    db.read(move |conn| {
        if hidden::tags(conn, user, library)?.contains(&id) {
            return Ok(None);
        }
        let mut found = find(conn, library, id)?;
        if let Some(tag) = &mut found {
            tag.lose(&hidden::losses(conn, user, library)?);
        }
        Ok(found)
    })
    .await?
    .map(Json)
    .ok_or_else(Problem::not_found)
}

/// The tags of `library`, of `tag_type` when there is one, whose names hold `q` when it holds
/// more than spaces.
fn source(library: i64, tag_type: Option<String>, q: Option<&str>) -> Source {
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
    // A name holds `q` where its sort key holds `q` folded the same way, so case and accents do
    // not count. Every tag the list walks is tested, which a library's tags are few enough for.
    if let Some(q) = q.map(tag_sort_key).filter(|q| !q.is_empty()) {
        source.filter += " AND instr(g.sort_key, ?) > 0";
        source.params.push(q.into_bytes().into());
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
