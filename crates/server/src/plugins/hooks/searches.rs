//! What a `searched` hook delivers: the searches a user who shares them with the plugin settled
//! on in a library, and the ones they asked it to forget, from the feed of search events
//! (`0005_activity.sql`, `requirements/search.md` §6).
//!
//! One event is one kind of thing, so a forget always reaches the plugin after the search it
//! names. A search removed before it was delivered is never sent: the feed names searches, and
//! their words live only in `recent_searches`. What a search found is kept on its event.

use jewelcase_plugins::{Event, Found, ResultKind, Search, Selected};
use rusqlite::{Connection, OptionalExtension, params};

use super::BATCH;

/// Which events reach the plugin `?4` from those of user `?2` in library `?1` after position
/// `?3`, as `e`: their searches and forgets while they share with it since `?5`, and stops
/// meant for it. `?5` is null while they do not share.
pub const FOR_PLUGIN: &str = "e.library_id = ?1 AND e.user_id = ?2 AND e.seq > ?3 \
     AND ((e.kind <> 'stopped' AND e.created_at >= ?5) \
          OR (e.kind = 'stopped' AND e.plugin_id = ?4))";

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
    let rows: Vec<(i64, String, Option<i64>)> = conn
        .prepare_cached(&format!(
            "SELECT e.seq, e.kind, e.search_id FROM search_events e WHERE {FOR_PLUGIN} \
             ORDER BY e.seq LIMIT ?6"
        ))?
        .query_map(params![library, user, position, plugin, shared_at, BATCH], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let Some((_, first, _)) = rows.first() else { return Ok((position, None)) };
    // The run of events of the same kind as the first.
    let kind = first.clone();
    let run: Vec<_> = rows.iter().take_while(|(_, k, _)| *k == kind).collect();
    let next = run.last().map_or(position, |(seq, ..)| *seq);
    let event = match kind.as_str() {
        "searched" => {
            let found = run
                .iter()
                .map(|(seq, ..)| search(conn, *seq))
                .filter_map(Result::transpose)
                .collect::<rusqlite::Result<Vec<_>>>()?;
            (!found.is_empty()).then_some(Event::Searched(found))
        }
        "forgotten" => {
            let ids = run.iter().filter_map(|(_, _, search)| search.map(|id| id as u64));
            Some(Event::SearchesForgotten(Some(ids.collect())))
        }
        _ => Some(Event::SearchesForgotten(None)),
    };
    Ok((next, event))
}

/// The search that the `searched` event `seq` names, with what it found, if the search is still
/// among its user's recent searches in the event's library.
fn search(conn: &Connection, seq: i64) -> rusqlite::Result<Option<Search>> {
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
        let selected_id: Option<i64> = row.get(6)?;
        Ok(Search {
            id: row.get::<_, i64>(0)? as u64,
            query: row.get(1)?,
            found: Found { tracks: row.get(2)?, albums: row.get(3)?, artists: row.get(4)? },
            selected: kind.zip(selected_id).map(|(kind, id)| Selected { kind, id: id as u64 }),
            searched_at_ms: row.get::<_, i64>(7)? as u64,
        })
    })
    .optional()
}
