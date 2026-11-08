//! Tags: the type list, the listing and a single tag (`listTagTypes`, `listTags` and `getTag`,
//! `api/openapi.yaml`).

use jewelcase_core::tag_sort_key;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::Id;
use crate::page::{Page, Request, Sort, Source, Total, Unknown};

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
pub enum TagSort {
    #[default]
    Name,
    TrackCount,
}

impl TagSort {
    /// The keyset this sort pages by. Each one's columns follow `library_id` and `type` in one
    /// of `tags`' indexes, so a list of one type walks it; a list of every type sorts the
    /// library's tags, of which there are few beside its tracks. Every tag has a sort key by the
    /// time its batch commits.
    pub fn sort(self) -> &'static Sort {
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

/// Each type the library has a tag of, with its count of tags and the name a plugin's type is
/// shown by, in key order. A tag goes with the last track that carries it, so every type listed
/// has music behind it.
pub fn types(conn: &Connection, library: i64) -> rusqlite::Result<Vec<TagType>> {
    conn.prepare_cached(
        "SELECT used.type, kind.name, used.tags \
         FROM (SELECT type, count(*) AS tags FROM tags WHERE library_id = ?1 GROUP BY type) \
         used JOIN tag_types kind ON kind.key = used.type ORDER BY used.type",
    )?
    .query_map([library], |row| {
        Ok(TagType { key: row.get(0)?, name: row.get(1)?, tag_count: row.get(2)? })
    })?
    .collect()
}

/// The library's tags, of `tag_type` when there is one and with `q` in their names when it holds
/// more than spaces, as the page `request` asks for. A type the library has no tag of lists none.
pub fn list(
    conn: &Connection,
    library: i64,
    request: Request,
    tag_type: Option<String>,
    q: Option<&str>,
) -> rusqlite::Result<Page<Tag>> {
    request.read(conn, &source(library, tag_type, q), tag)
}

/// The tags of `library`, of `tag_type` when there is one, whose names hold `q` when it holds
/// more than spaces.
pub fn source(library: i64, tag_type: Option<String>, q: Option<&str>) -> Source {
    let mut source = Source {
        select: SELECT,
        from: "tags g".to_owned(),
        joins: "",
        filter: "g.library_id = ?".to_owned(),
        params: vec![library.into()],
        total: Total::Counted,
        counting: None,
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
pub fn find(conn: &Connection, library: i64, id: i64) -> rusqlite::Result<Option<Tag>> {
    let sql =
        format!("SELECT {} FROM tags g WHERE g.library_id = ?1 AND g.id = ?2", SELECT.join(", "));
    conn.prepare_cached(&sql)?.query_row([library, id], tag).optional()
}
