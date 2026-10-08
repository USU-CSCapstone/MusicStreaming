//! What a `searched` hook delivers: the searches a user who shares them with the plugin settled
//! on in a library, and the ones they asked it to forget, from the feed of search events
//! (`0016_plugin_searched.sql`, `requirements/search.md` §6).
//!
//! One event is one kind of thing, so a forget always reaches the plugin after the search it
//! names. A search removed before it was delivered is never sent: the feed names searches, and
//! their words live only in `recent_searches`.

use jewelcase_plugins::{Event, Found, ResultKind, Search, Selected};
use rusqlite::{Connection, OptionalExtension, params};

use super::BATCH;

/// Which events reach the plugin `?4` from those of user `?2` in library `?1` after position
/// `?3`, as `e`: their searches and forgets while they share with it since `?5`, and stops
/// meant for it. `?5` is null while they do not share.
pub const FOR_PLUGIN: &str = "e.library_id = ?1 AND e.user_id = ?2 AND e.seq > ?3 \
     AND ((e.kind <> 'stopped' AND e.at >= ?5) OR (e.kind = 'stopped' AND e.plugin_id = ?4))";

/// What `plugin` is to be told next of `user`'s searches in `library`, after `position`: the
/// position after it, and the event, if any of it is still worth sending.
pub fn searches(
    conn: &Connection,
    plugin: &str,
    library: i64,
    user: i64,
    position: i64,
) -> rusqlite::Result<(i64, Option<Event>)> {
    let shared_at: Option<i64> = conn
        .prepare_cached(
            "SELECT shared_at FROM plugin_search_sharing WHERE plugin_id = ?1 AND user_id = ?2",
        )?
        .query_row(params![plugin, user], |row| row.get(0))
        .optional()?;
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
    let ids = run.iter().filter_map(|(_, _, search)| *search);
    let event = match kind.as_str() {
        "searched" => {
            let found = ids
                .map(|id| search(conn, user, library, id))
                .filter_map(Result::transpose)
                .collect::<rusqlite::Result<Vec<_>>>()?;
            (!found.is_empty()).then_some(Event::Searched(found))
        }
        "forgotten" => Some(Event::SearchesForgotten(Some(ids.map(|id| id as u64).collect()))),
        _ => Some(Event::SearchesForgotten(None)),
    };
    Ok((next, event))
}

/// `user`'s search `id` in `library`, if it is still there.
fn search(conn: &Connection, user: i64, library: i64, id: i64) -> rusqlite::Result<Option<Search>> {
    conn.prepare_cached(
        "SELECT query, found_tracks, found_albums, found_artists, selected_type, selected_id, \
                searched_at FROM recent_searches WHERE id = ?1 AND user_id = ?2 AND library_id = ?3",
    )?
    .query_row(params![id, user, library], |row| {
        let kind = match row.get::<_, Option<String>>(4)?.as_deref() {
            Some("track") => Some(ResultKind::Track),
            Some("album") => Some(ResultKind::Album),
            Some("artist") => Some(ResultKind::Artist),
            _ => None,
        };
        let selected_id: Option<i64> = row.get(5)?;
        Ok(Search {
            id: id as u64,
            query: row.get(0)?,
            found: Found { tracks: row.get(1)?, albums: row.get(2)?, artists: row.get(3)? },
            selected: kind.zip(selected_id).map(|(kind, id)| Selected { kind, id: id as u64 }),
            searched_at: row.get::<_, i64>(6)? as u64,
        })
    })
    .optional()
}
