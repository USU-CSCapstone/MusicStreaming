//! What a user hid, as every read leaves it out: a hidden track is gone from the user's library
//! and shown only in Hidden Songs (`requirements/conventions.md` §8), and what only hidden tracks
//! make up goes with them. The server and a device read the same `hidden_tracks`, which a device
//! keeps from its account feed, so both answer alike.

use std::collections::HashSet;

use rusqlite::Connection;
use rusqlite::types::Value;

use crate::page::{Order, Page, Request, Sort, Source, Total, Unknown};
use crate::tracks::{self, TrackSummary};

mod losses;

pub use losses::{AlbumLoss, ArtistLoss, LibraryLoss, Losses, TagLoss, losses, playlists};

/// Hidden Songs' order, by when each track was hidden, through `hidden_tracks_by_time`.
pub const HIDDEN_AT: Sort =
    Sort { label: "hidden.time", columns: &["h.hidden_at", "h.track_id"], unknown: Unknown::Never };

/// Hidden Songs: the tracks `user` hid in `library`, newest first unless `request` says `asc`,
/// as the page `request` asks for. Every one is marked hidden, and none liked, since a hidden
/// track is never liked.
pub fn list(
    conn: &Connection,
    user: i64,
    library: i64,
    request: Request,
) -> rusqlite::Result<Page<TrackSummary>> {
    // Counted for every page: a user hides few of a library's tracks, all in one index range.
    let source = Source {
        select: tracks::SELECT,
        from: "hidden_tracks h CROSS JOIN tracks t ON t.library_id = h.library_id \
               AND t.id = h.track_id"
            .to_owned(),
        joins: tracks::JOINS,
        filter: "h.user_id = ? AND h.library_id = ?".to_owned(),
        params: vec![Value::from(user), Value::from(library)],
        total: Total::Counted,
        counting: None,
        back: None,
        walk: None,
    };
    let mut page = request.read(conn, &source, tracks::list_summary)?;
    tracks::mark_hidden(conn, user, library, &mut page.items)?;
    Ok(page)
}

/// Hidden Songs' default direction: newest first.
pub const DEFAULT_ORDER: Order = Order::Desc;

/// The tracks `user` hid in `library`. A user hides few of a library's tracks, so they are held
/// as a set, which a search consults for each match.
pub fn ids(conn: &Connection, user: i64, library: i64) -> rusqlite::Result<HashSet<i64>> {
    conn.prepare_cached(
        "SELECT track_id FROM hidden_tracks WHERE user_id = ?1 AND library_id = ?2",
    )?
    .query_map([user, library], |row| row.get(0))?
    .collect()
}

/// `source`, a list of tracks `t`, without the tracks `user` hid in `library`
/// ([`leave_out_ids`]).
pub fn leave_out(
    conn: &Connection,
    source: &mut Source,
    user: i64,
    library: i64,
) -> rusqlite::Result<()> {
    let hidden: Vec<i64> = ids(conn, user, library)?.into_iter().collect();
    leave_out_ids(conn, source, "t.id", &hidden)
}

/// `source`, a list whose rows `column` names, such as `al.id`, without the rows `ids`, which
/// are few: what the caller hid, or hid whole. Each row is one lookup in them. A stored total
/// loses those of them the list holds, counted by reading the list for those alone, so the total
/// stays a stored count. Over a table, or a lead of one table such as a tag's, that is a lookup
/// per ID, so it costs what they do. A lead SQLite cannot look into by ID, a compound one such as
/// an artist's owned and featured albums together, is read through once instead, as the page
/// itself reads it to sort it, so the count costs no more than the page. A count read from the
/// tag links ([`Counting`](crate::page::Counting)) leaves them out of the links' owners, `lead.id`.
/// Nothing changes where there are none, the common case.
pub fn leave_out_ids(
    conn: &Connection,
    source: &mut Source,
    column: &str,
    ids: &[i64],
) -> rusqlite::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    // In order, so the same IDs make the same parameters, which a counted total is kept by.
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    let ids = Value::from(serde_json::to_string(&ids).expect("IDs serialize"));
    if let Total::Stored(total) = source.total {
        let sql = format!(
            "SELECT count(*) FROM {} WHERE {} AND {column} IN (SELECT value FROM json_each(?))",
            source.from, source.filter
        );
        let params = source.params.iter().cloned().chain([ids.clone()]);
        let held: i64 = conn
            .prepare_cached(&sql)?
            .query_row(rusqlite::params_from_iter(params), |row| row.get(0))?;
        source.total = Total::Stored(total - held);
    }
    if let Some(counting) = &mut source.counting {
        counting.filter += " AND lead.id NOT IN (SELECT value FROM json_each(?))";
        counting.params.push(ids.clone());
    }
    exclude(source, column, &ids);
    Ok(())
}

/// `source` and what a page of it reads instead if its walk runs over
/// ([`Walk`](crate::page::Walk)), without the rows of `column` that are in `ids`, a JSON array. The
/// walk's own window edges need no such condition: they only bound how far the walk reads.
fn exclude(source: &mut Source, column: &str, ids: &Value) {
    source.filter += &format!(" AND {column} NOT IN (SELECT value FROM json_each(?))");
    source.params.push(ids.clone());
    if let Some(walk) = &mut source.walk {
        exclude(&mut walk.lead, column, ids);
    }
}

/// The user's hidden tracks, as the queries of [`albums`], [`artists`] and [`tags`] start from.
const HID: &str =
    "WITH hid AS (SELECT track_id FROM hidden_tracks WHERE user_id = ?1 AND library_id = ?2) ";

/// The albums of `library` every track of which `user` hid, which are gone from their library
/// with them (`requirements/conventions.md` §8). Only the albums of hidden tracks can be, and
/// each is kept unless a probe of `tracks_by_album_order` finds a track left, which it stops at.
fn albums(conn: &Connection, user: i64, library: i64) -> rusqlite::Result<Vec<i64>> {
    wholly(
        conn,
        user,
        library,
        "SELECT DISTINCT t.album_id FROM hid CROSS JOIN tracks t ON t.id = hid.track_id \
         WHERE NOT EXISTS (SELECT 1 FROM tracks o \
                           WHERE o.album_id = t.album_id AND o.id NOT IN hid)",
    )
}

/// The artists of `library` every track of which `user` hid: those that credit them and those
/// on their albums, each probed through its index until one is found left, as for [`albums`].
fn artists(conn: &Connection, user: i64, library: i64) -> rusqlite::Result<Vec<i64>> {
    wholly(
        conn,
        user,
        library,
        "SELECT c.artist_id FROM ( \
           SELECT ta.artist_id FROM hid CROSS JOIN track_artists ta ON ta.track_id = hid.track_id \
           UNION \
           SELECT aa.artist_id FROM hid CROSS JOIN tracks t ON t.id = hid.track_id \
             CROSS JOIN album_artists aa ON aa.album_id = t.album_id) c \
         WHERE c.artist_id IS NOT NULL \
           AND NOT EXISTS (SELECT 1 FROM track_artists o \
                           WHERE o.artist_id = c.artist_id AND o.track_id NOT IN hid) \
           AND NOT EXISTS (SELECT 1 FROM album_artists aa CROSS JOIN tracks o \
                             ON o.album_id = aa.album_id \
                           WHERE aa.artist_id = c.artist_id AND o.id NOT IN hid)",
    )
}

/// The tags of `library` every track carrying which `user` hid, each probed through
/// `track_tags_by_tag`, as for [`albums`].
pub fn tags(conn: &Connection, user: i64, library: i64) -> rusqlite::Result<Vec<i64>> {
    wholly(
        conn,
        user,
        library,
        "SELECT DISTINCT tt.tag_id FROM hid CROSS JOIN track_tags tt ON tt.track_id = hid.track_id \
         WHERE NOT EXISTS (SELECT 1 FROM track_tags o \
                           WHERE o.tag_id = tt.tag_id AND o.track_id NOT IN hid)",
    )
}

/// The IDs `select`, a query after [`HID`], finds for `user` in `library`.
fn wholly(conn: &Connection, user: i64, library: i64, select: &str) -> rusqlite::Result<Vec<i64>> {
    conn.prepare_cached(&format!("{HID}{select}"))?
        .query_map([user, library], |row| row.get(0))?
        .collect()
}
