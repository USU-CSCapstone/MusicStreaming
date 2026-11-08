//! Playlists: the listing, a single playlist, and its items (`listPlaylists`, `getPlaylist` and
//! `listPlaylistItems`, `api/openapi.yaml`).
//!
//! A playlist is its user's own, in one library (`design/database.md` §6): every read is scoped
//! to `user` as well as the library, so another account's playlist is as absent as one that
//! never was. On a device `user` is the signed-in account, the only one whose playlists it holds.

mod representation;

use std::collections::HashMap;
use std::fmt;

use rusqlite::types::Value;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use jewelcase_core::words;

pub use representation::{Playlist, SELECT, get, header, playlist};

use crate::by_ids::by_ids;
use crate::page::{Page, Request, Sort, Source, Total, Unknown};
use crate::time::serialize_rfc3339;
use crate::tracks::{self, TrackSummary};
use crate::{Invalid, cursor, page};

/// What names the items list in its cursors.
const LABEL: &str = "playlists.items";

/// A playlist's items after a key, in order: one range of the primary key.
pub const ITEMS: &str = "SELECT key, track_id, added_at FROM playlist_items \
                     WHERE playlist_id = ?1 AND key > ?2 ORDER BY key LIMIT ?3";

/// The `sort` values `listPlaylists` supports.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaylistSort {
    Name,
    Created,
    #[default]
    Modified,
    // The spec's `playCount` needs listening data, which is not kept per playlist yet, so it
    // answers `422`.
}

impl PlaylistSort {
    /// The keyset this sort pages by. A pinned playlist's `pin_rank` is 1 and the rest's NULL,
    /// the unknown part of the list, so the pinned come first in either order. Each sort's
    /// columns are one of `playlists`' indexes after `user_id` and `library_id`.
    pub fn sort(self) -> &'static Sort {
        match self {
            PlaylistSort::Name => &Sort {
                label: "playlists.name",
                columns: &["p.pin_rank", "p.sort_key", "p.id"],
                unknown: Unknown::Null,
            },
            PlaylistSort::Created => &Sort {
                label: "playlists.created",
                columns: &["p.pin_rank", "p.created_at", "p.id"],
                unknown: Unknown::Null,
            },
            PlaylistSort::Modified => &Sort {
                label: "playlists.modified",
                columns: &["p.pin_rank", "p.modified_at", "p.id"],
                unknown: Unknown::Null,
            },
        }
    }
}

/// The playlists `user` has in `library`, those whose title holds all of `q`'s words, in order
/// if `q` has any.
pub fn source(user: i64, library: i64, q: Option<&str>) -> Source {
    let mut filter = "p.user_id = ? AND p.library_id = ?".to_owned();
    let mut params = vec![Value::from(user), Value::from(library)];
    let folded = words(q.unwrap_or("")).join(" ");
    if !folded.is_empty() {
        filter += " AND instr(p.folded, ?) > 0";
        params.push(Value::from(folded));
    }
    // Counted for every page: a user has hundreds of playlists at most, all in one index range.
    Source {
        select: SELECT,
        from: "playlists p".to_owned(),
        joins: "",
        filter,
        params,
        total: Total::Counted,
        counting: None,
    }
}

/// The playlists `user` has in `library`, those whose title holds all of `q`'s words, as the page
/// `request` asks for.
pub fn list(
    conn: &Connection,
    user: i64,
    library: i64,
    request: Request,
    q: Option<&str>,
) -> rusqlite::Result<Page<Playlist>> {
    request.read(conn, &source(user, library, q), playlist)
}

/// A playlist's version and every one of its items, as (track, added at).
pub type AllItems = (i64, Vec<(i64, i64)>);

/// The version of `user`'s playlist `id` in `library` and every one of its items in the user's
/// order, as (track, added at), a duplicate as often as it occurs and a missing track in place,
/// from one read. `None` if the playlist is not theirs in the library.
pub fn all_items(
    conn: &Connection,
    user: i64,
    library: i64,
    id: i64,
) -> rusqlite::Result<Option<AllItems>> {
    let Some((version, _)) = header(conn, user, library, id)? else { return Ok(None) };
    // No limit: a negative one reads every item.
    let items = conn
        .prepare_cached(ITEMS)?
        .query_map(params![id, i64::MIN, -1], |row| Ok((row.get(1)?, row.get(2)?)))?
        .collect::<Result<_, _>>()?;
    Ok(Some((version, items)))
}

/// The spec's `PlaylistItem`; an item of an external track is not possible yet.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    track: TrackSummary,
    #[serde(serialize_with = "serialize_rfc3339")]
    added_at: i64,
}

/// The spec's `PlaylistItemPage`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemPage {
    items: Vec<Item>,
    next_cursor: Option<String>,
    total: i64,
    version: i64,
}

/// Why a playlist's items could not be listed.
#[derive(Debug)]
pub enum ItemsError {
    Invalid(Invalid),
    /// The playlist is not the user's own in the library.
    NotFound,
    /// The cursor is from another version of the playlist: the list starts again.
    Expired,
    /// An item's track is not in the catalog, so the page would come up short against the
    /// playlist's version and total. Never on the server, where retention keeps every track an
    /// item names; on a device, a track the library feed has not brought yet.
    Incomplete,
    Sql(rusqlite::Error),
}

/// The message a caller can show or log.
impl fmt::Display for ItemsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ItemsError::Invalid(Invalid(why)) => f.write_str(why),
            ItemsError::NotFound => f.write_str("no such playlist"),
            ItemsError::Expired => f.write_str("the playlist changed since the first page"),
            ItemsError::Incomplete => f.write_str("a playlist item's track is not here yet"),
            ItemsError::Sql(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ItemsError {}

/// Lets `?` pass a rejected request on as [`ItemsError::Invalid`].
impl From<Invalid> for ItemsError {
    fn from(invalid: Invalid) -> ItemsError {
        ItemsError::Invalid(invalid)
    }
}

/// Lets `?` pass a database error on as [`ItemsError::Sql`].
impl From<rusqlite::Error> for ItemsError {
    fn from(error: rusqlite::Error) -> ItemsError {
        ItemsError::Sql(error)
    }
}

/// One page of `user`'s playlist `id` in `library`, in the user's order, a duplicate as often as
/// it occurs and a missing track in place. The version check, the rows and the summaries come
/// from one read, so a page is of one version. A cursor from another version is
/// [`ItemsError::Expired`], and an item whose track row is absent [`ItemsError::Incomplete`].
pub fn items(
    conn: &Connection,
    user: i64,
    library: i64,
    id: i64,
    cursor: Option<&str>,
    limit: Option<u32>,
) -> Result<ItemPage, ItemsError> {
    let limit = page::limit(limit)?;
    let after = cursor
        .map(|cursor| cursor::decode(cursor, LABEL, 2))
        .transpose()?
        .map(|values| match values[..] {
            [Value::Integer(version), Value::Integer(key)] => Ok((version, key)),
            _ => Err(Invalid("cursor is not valid for this list".to_owned())),
        })
        .transpose()?;
    let (version, total) = header(conn, user, library, id)?.ok_or(ItemsError::NotFound)?;
    if after.is_some_and(|(cursor_version, _)| cursor_version != version) {
        return Err(ItemsError::Expired);
    }
    let start = after.map_or(i64::MIN, |(_, key)| key);
    // One more than the page, to know whether another follows. Keyset on the primary key.
    let mut rows: Vec<(i64, i64, i64)> = conn
        .prepare_cached(ITEMS)?
        .query_map(params![id, start, limit + 1], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = rows
        .last()
        .filter(|_| more)
        .map(|(key, ..)| cursor::encode(LABEL, &[version.into(), (*key).into()]));
    let mut wanted: Vec<i64> = rows.iter().map(|row| row.1).collect();
    wanted.sort_unstable();
    wanted.dedup();
    let summaries: HashMap<i64, TrackSummary> =
        by_ids(conn, tracks::SELECT, "tracks t", tracks::JOINS, library, &wanted, |row| {
            let track = tracks::list_summary(row)?;
            Ok((row.get::<_, i64>(0)?, track))
        })?
        .into_iter()
        .collect();
    // Leaving an item out would shift every position after it, which edits address items by.
    let items = rows
        .iter()
        .map(|(_, track, added_at)| {
            let track = summaries.get(track).ok_or(ItemsError::Incomplete)?.clone();
            Ok(Item { track, added_at: *added_at })
        })
        .collect::<Result<_, ItemsError>>()?;
    Ok(ItemPage { items, next_cursor, total, version })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A device database whose library 1 holds tracks 101 and 103 but not yet 102, and user 7's
    /// playlist 10 at version 4, whose items are tracks 101, 102 and 103 in that order.
    pub(crate) fn behind_the_library_feed() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::create_schema(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO tracks (id, library_id, album_id, file_size, title, sort_key,
                 artist_sort_key, album_sort_key, codec, container, lossless, sample_rate_hz,
                 channels, duration_us, added_at)
             VALUES (101, 1, 1, 1, 'A', x'61', x'', x'', 'flac', 'flac', 1, 44100, 2, 1, 0),
                    (103, 1, 1, 1, 'C', x'63', x'', x'', 'flac', 'flac', 1, 44100, 2, 1, 0);
             INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, version,
                 created_at, modified_at, track_count)
             VALUES (10, 1, 7, 'Mix', 'mix', x'6d6978', 4, 0, 0, 3);
             INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at)
             VALUES (10, 1, 1, 101, 0), (10, 2, 1, 102, 0), (10, 3, 1, 103, 0);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn a_device_refuses_items_whose_track_has_not_arrived() {
        let conn = behind_the_library_feed();
        let result = items(&conn, 7, 1, 10, None, None);
        assert!(matches!(result, Err(ItemsError::Incomplete)), "{:?}", result.err());
    }
}
