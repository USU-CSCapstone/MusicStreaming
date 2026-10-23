//! What a `searched` hook delivers: the searches a user who shares them with the plugin settled
//! on in a library, and the ones they asked it to forget, from the feed of search events
//! (`0005_activity.sql`, `requirements/search.md` §6).
//!
//! One event is one kind of thing, so a forget always reaches the plugin after the search it
//! names. A search removed before it was delivered is never sent: the feed names searches, and
//! their words live only in `recent_searches`. What a search found is kept on its event.
//!
//! A search is sent only while its user can be acted for there ([`ACTS`]). A forget names its
//! user and carries nothing else of theirs, so it is sent whether or not they can: one written
//! before they lost the library or were suspended still reaches the plugin, and a search held
//! back for that is passed over.

use jewelcase_plugins::{Event, Forgotten, Found, ResultKind, Search, Selected};
use rusqlite::{Connection, OptionalExtension, params};

use super::BATCH;
use crate::db::reaches;

/// Whether the user of the event `e` can be acted for in its library: their account is active,
/// and they reach the library.
pub const ACTS: &str = concat!(
    "((SELECT a.status = 'active' FROM users a WHERE a.id = e.user_id) AND ",
    reaches!(
        admin: "(SELECT a.role <> 'user' FROM users a WHERE a.id = e.user_id)",
        user: "e.user_id",
        library: "e.library_id"
    ),
    ")"
);

/// What `plugin` is to be told next of `user`'s searches in `library`, after `position`: the
/// position after it, and the event, if any of it is still worth sending.
pub fn searches(
    conn: &Connection,
    plugin: &str,
    library: i64,
    user: i64,
    position: i64,
) -> rusqlite::Result<(i64, Option<Event>)> {
    // Null while they do not share, and when they are not connected at all.
    let shared_at: Option<i64> = conn
        .prepare_cached(
            "SELECT searches_shared_at FROM plugin_connections \
             WHERE plugin_id = ?1 AND user_id = ?2",
        )?
        .query_row(params![plugin, user], |row| row.get(0))
        .optional()?
        .flatten();
    // Their searches while they share with it and can be acted for, their forgets while they
    // share with it, and stops meant for it. `shared_at` is null while they do not share.
    let rows: Vec<(i64, String, Option<u64>)> = conn
        .prepare_cached(&format!(
            "SELECT e.seq, e.kind, e.search_id FROM search_events e \
             WHERE e.library_id = ?1 AND e.user_id = ?2 AND e.seq > ?3 \
             AND ((e.kind = 'searched' AND e.created_at >= ?5 AND {ACTS}) \
                  OR (e.kind = 'forgotten' AND e.created_at >= ?5) \
                  OR (e.kind = 'stopped' AND e.plugin_id = ?4)) \
             ORDER BY e.seq LIMIT ?6"
        ))?
        .query_map(params![library, user, position, plugin, shared_at, BATCH], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let Some((_, first, _)) = rows.first() else { return Ok((position, None)) };
    // The user as the contract names them, refused as rusqlite refuses reading a negative ID.
    let user =
        u64::try_from(user).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, user))?;
    // The run of events of the same kind as the first.
    let kind = first.clone();
    let run: Vec<_> = rows.iter().take_while(|(_, k, _)| *k == kind).collect();
    let next = run.last().map_or(position, |(seq, ..)| *seq);
    let event = match kind.as_str() {
        "searched" => {
            let found = run
                .iter()
                .map(|(seq, ..)| search(conn, *seq, user))
                .filter_map(Result::transpose)
                .collect::<rusqlite::Result<Vec<_>>>()?;
            (!found.is_empty()).then_some(Event::Searched(found))
        }
        "forgotten" => {
            let ids = run.iter().filter_map(|(_, _, search)| *search);
            Some(Event::SearchesForgotten(Forgotten { user, searches: Some(ids.collect()) }))
        }
        _ => Some(Event::SearchesForgotten(Forgotten { user, searches: None })),
    };
    Ok((next, event))
}

/// The search that the `searched` event `seq` names, with what it found, if the search is still
/// among its user's recent searches in the event's library. `user` is that user.
fn search(conn: &Connection, seq: i64, user: u64) -> rusqlite::Result<Option<Search>> {
    conn.prepare_cached(
        "SELECT r.id, r.query, e.found_tracks, e.found_albums, e.found_artists, r.selected_type, \
                r.selected_id, r.searched_at FROM search_events e \
         JOIN recent_searches r \
              ON r.id = e.search_id AND r.user_id = e.user_id AND r.library_id = e.library_id \
         WHERE e.seq = ?1",
    )?
    .query_row([seq], |row| {
        let kind = match row.get::<_, Option<String>>(5)?.as_deref() {
            Some("track") => Some(ResultKind::Track),
            Some("album") => Some(ResultKind::Album),
            Some("artist") => Some(ResultKind::Artist),
            _ => None,
        };
        let selected_id: Option<u64> = row.get(6)?;
        Ok(Search {
            id: row.get(0)?,
            user,
            query: row.get(1)?,
            found: Found { tracks: row.get(2)?, albums: row.get(3)?, artists: row.get(4)? },
            selected: kind.zip(selected_id).map(|(kind, id)| Selected { kind, id }),
            searched_at_ms: row.get(7)?,
        })
    })
    .optional()
}
