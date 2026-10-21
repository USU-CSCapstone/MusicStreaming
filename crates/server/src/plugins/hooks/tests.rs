//! What plugins receive from hooks: each hook delivered to a stand-in plugin, as
//! [`Plugins::deliver`] delivers it, and the whole path to the real plugins.

use std::sync::Arc;

use jewelcase_plugins::{Event, Permission};
use serde_json::json;

use super::due::{all, event, queries};
use super::{Hook, Ran, record};
use crate::db::Database;
use crate::db::testing::{User, access};
use crate::plugins::Plugins;
use crate::plugins::testing::{connect, database, enable, grant, install, manifest, plugins};

mod changes;
mod libraries;
mod listening;
mod outages;
mod real;
mod reinstall;
mod scans;
mod searches;

/// A plugin's answer to an event it handled.
const HANDLED: Result<(), &str> = Ok(());
const HOUR: i64 = 60 * 60_000;

/// A delivery to a stand-in plugin: the hook, library, and user it was due for, and the event
/// the plugin received, if there was anything to send.
#[derive(Debug)]
struct Received {
    hook: Hook,
    library: i64,
    user: i64,
    event: Option<Event>,
}

/// The catalog of [`database`] with `plugin` installed and enabled in library 1. It asks for
/// every hook and what each needs, with a schedule of an hour and settings users connect it
/// with, and is granted none of them.
async fn setup() -> (tempfile::TempDir, Arc<Database>, Arc<Plugins>) {
    use Permission::*;
    let (temp, db) = database().await;
    let mut p = manifest(
        "plugin",
        &[
            LibraryRead,
            TracksChanged,
            AlbumsChanged,
            ArtistsChanged,
            ScanFinished,
            Schedule,
            ListeningActivity,
            Played,
            Playing,
            SearchActivity,
            Searched,
        ],
    );
    let requests = p["permissions"].as_array_mut().unwrap();
    let schedule = requests.iter_mut().find(|r| r["permission"] == "schedule").unwrap();
    schedule["everyMinutes"] = json!(60);
    p["personalSettings"] = json!({ "properties": { "token": { "type": "string" } } });
    install(&db, p).await;
    let plugins = plugins(&temp, &db);
    enable(&plugins, "plugin", true).await;
    (temp, db, plugins)
}

/// Delivers every hook due at `now` to a stand-in plugin that answers each event with
/// `answer`, where an error is a failure of the plugin's own ([`Ran::Failed`]), and returns each
/// delivery.
async fn deliver(db: &Database, now: i64, answer: Result<(), &'static str>) -> Vec<Received> {
    match answer {
        Ok(()) => ran(db, now, Ran::Handled, "").await,
        Err(why) => ran(db, now, Ran::Failed, why).await,
    }
}

/// Delivers every hook due at `now` to a stand-in plugin whose run goes as `how`, saying
/// `summary`, and returns each delivery. This is [`Plugins::deliver`] with the plugin left out:
/// a delivery with nothing for the plugin moves on without reaching it, but is still returned,
/// so a hook found due with nothing to send shows.
async fn ran(db: &Database, now: i64, how: Ran, summary: &'static str) -> Vec<Received> {
    db.write(move |tx| {
        let mut received = Vec::new();
        for due in all(tx, now)? {
            let (next, event) = event(tx, &due, now)?;
            let how = if event.is_none() { Ran::Handled } else { how };
            record(tx, &due, next, how, summary, now)?;
            let (hook, library, user) = (due.hook, due.library, due.user);
            received.push(Received { hook, library, user, event });
        }
        Ok(received)
    })
    .await
    .unwrap()
}

/// The steps of the plan for the query that finds `hook` due, which runs every few seconds.
async fn plan(db: &Database, hook: Hook) -> Vec<String> {
    let (_, sql) = queries().into_iter().find(|(h, _)| *h == hook).unwrap();
    db.read(move |conn| {
        conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
            .query_map(rusqlite::params![0, hook.name()], |row| row.get(3))?
            .collect()
    })
    .await
    .unwrap()
}

/// The event of the one delivery `received`, which came for `user` in library 1.
fn one(received: Vec<Received>, user: i64) -> Event {
    match <[Received; 1]>::try_from(received) {
        Ok([Received { library: 1, user: to, event: Some(event), .. }]) if to == user => event,
        other => panic!("not one event for user {user}: {other:?}"),
    }
}

/// Each delivery in `received` as its user and the tracks its event names, in user order.
/// Panics on an event that is neither played nor playing.
fn tracks_by_user(received: Vec<Received>) -> Vec<(i64, Vec<u64>)> {
    let mut tracks: Vec<_> = received
        .into_iter()
        .map(|delivery| {
            let tracks = match delivery.event {
                Some(Event::Played(plays)) => plays.iter().map(|p| p.track.id).collect(),
                Some(Event::Playing(now)) => vec![now.track.id],
                None => Vec::new(),
                Some(other) => panic!("not a listening event: {other:?}"),
            };
            (delivery.user, tracks)
        })
        .collect();
    tracks.sort();
    tracks
}

/// Adds `rows` to the library change feed, each `(library, entity type, entity, op, changed at)`.
async fn changes(db: &Database, rows: &'static str) {
    db.write(move |tx| {
        tx.execute_batch(&format!(
            "INSERT INTO library_changes (library_id, entity_type, entity_id, op, changed_at) VALUES {rows}"
        ))
    })
    .await
    .unwrap();
}

/// `plugin` granted `plugin_wide` and reading library 1, which users 7 and 8 reach; user 7
/// connected it at time 100, and user 8 has not.
async fn connected(plugins: &Plugins, db: &Database, plugin_wide: &[Permission]) {
    grant(plugins, "plugin", plugin_wide, &[Permission::LibraryRead]).await;
    db.write(|tx| {
        for user in [7, 8] {
            User::new(user, "user").insert(tx)?;
            access(tx, user, 1)?;
        }
        Ok(())
    })
    .await
    .unwrap();
    connect(db, "plugin", 7, json!({}), 100).await;
}

/// User `user` plays track `track` in library 1 as play `id`, starting at `at`: reported as it
/// starts, then twice as ended with `end`.
async fn play(db: &Database, id: &'static str, user: i64, track: i64, at: i64, end: &'static str) {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO plays (id, user_id, library_id, track_id, title, artists, artist_keys, \
                                album_title_key, album_artists_key, genre_keys, duration_us, \
                                started_at, listen_time_ms, context, origin, updated_at) \
             VALUES (?1, ?2, 1, ?3, 'A', '[]', '[]', '', '', '[]', 0, ?4, 0, '{}', 'context', ?4)",
            rusqlite::params![id, user, track, at],
        )?;
        let ended =
            "UPDATE plays SET ended = ?2, listen_time_ms = 30000 WHERE user_id = ?3 AND id = ?1";
        tx.execute(ended, rusqlite::params![id, end, user])?;
        // Reported again: it ended once.
        tx.execute(ended, rusqlite::params![id, end, user])
    })
    .await
    .unwrap();
}

/// User `user` starts track `track` in library 1 as play `id`, first reported at `at`, and has
/// not finished it.
async fn start(db: &Database, id: &'static str, user: i64, track: i64, at: i64) {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO plays (id, user_id, library_id, track_id, title, artists, artist_keys, \
                                album_title_key, album_artists_key, genre_keys, duration_us, \
                                started_at, listen_time_ms, context, origin, updated_at) \
             VALUES (?1, ?2, 1, ?3, 'A', '[]', '[]', '', '', '[]', 0, ?4, 0, '{}', 'context', ?4)",
            rusqlite::params![id, user, track, at],
        )
    })
    .await
    .unwrap();
}
