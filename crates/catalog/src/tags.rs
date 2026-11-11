//! Tags: the type list, the listing and a single tag (`listTagTypes`, `listTags` and `getTag`,
//! `api/openapi.yaml`).

use jewelcase_core::tag_sort_key;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::Id;
use crate::hidden::{self, Losses};
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

/// Each type the library has a tag of that `user` did not hide whole, with its count of such
/// tags and the name a plugin's type is shown by, in key order. A tag goes with the last track
/// that carries it, so every type listed has music behind it.
pub fn types(conn: &Connection, user: i64, library: i64) -> rusqlite::Result<Vec<TagType>> {
    let whole = serde_json::to_string(&hidden::tags(conn, user, library)?).expect("IDs serialize");
    conn.prepare_cached(
        "SELECT used.type, kind.name, used.tags \
         FROM (SELECT type, count(*) AS tags FROM tags WHERE library_id = ?1 \
               AND id NOT IN (SELECT value FROM json_each(?2)) GROUP BY type) \
         used JOIN tag_types kind ON kind.key = used.type ORDER BY used.type",
    )?
    .query_map(rusqlite::params![library, whole], |row| {
        Ok(TagType { key: row.get(0)?, name: row.get(1)?, tag_count: row.get(2)? })
    })?
    .collect()
}

/// The library's tags, of `tag_type` when there is one and with `q` in their names when it holds
/// more than spaces, as the page `request` asks for `user`: without those they hid whole, each
/// counting what they did not hide. A type the library has no tag of lists none.
pub fn list(
    conn: &Connection,
    user: i64,
    library: i64,
    request: Request,
    tag_type: Option<String>,
    q: Option<&str>,
) -> rusqlite::Result<Page<Tag>> {
    let mut source = source(library, tag_type, q);
    hidden::leave_out_ids(conn, &mut source, "g.id", &hidden::tags(conn, user, library)?)?;
    let mut page = request.read(conn, &source, tag)?;
    let losses = hidden::losses(conn, user, library)?;
    page.items.iter_mut().for_each(|item| item.lose(&losses));
    Ok(page)
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
        back: None,
        walk: None,
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

/// Tag `id` of `library` as `user` sees it, or `None` when the library has no such tag or they
/// hid it whole.
pub fn find(conn: &Connection, user: i64, library: i64, id: i64) -> rusqlite::Result<Option<Tag>> {
    if hidden::tags(conn, user, library)?.contains(&id) {
        return Ok(None);
    }
    let sql =
        format!("SELECT {} FROM tags g WHERE g.library_id = ?1 AND g.id = ?2", SELECT.join(", "));
    let mut found = conn.prepare_cached(&sql)?.query_row([library, id], tag).optional()?;
    if let Some(tag) = &mut found {
        tag.lose(&hidden::losses(conn, user, library)?);
    }
    Ok(found)
}
