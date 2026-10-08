use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use jewelcase_plugins::Permission;
use rusqlite::Connection;

use super::super::Plugins;
use super::super::testing::database;
use super::due::{all, batch, event};
use super::playing::{FRESH_MS, playing};
use super::plays::plays;
use super::searches::searches;
use super::{BACKOFF_MS, Due, Hook, MAX_FAILURES, record};
use crate::db::Database;

/// Plugin `p` enabled in library 1 with the hook and read access granted there.
async fn subscribed(db: &Database) {
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO plugin_library_grants VALUES ('p', 1, 'tracksChanged'), ('p', 1, 'libraryRead');
             INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES ('p', 1, 1);",
        )
    })
    .await
    .unwrap();
}

async fn changes(db: &Database, rows: &'static str) {
    db.write(move |tx| {
        tx.execute_batch(&format!(
            "INSERT INTO library_changes (library_id, entity_type, entity_id, op, at) VALUES {rows}"
        ))
    })
    .await
    .unwrap();
}

async fn run<T: Send + 'static>(
    db: &Database,
    f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
) -> T {
    db.read(f).await.unwrap()
}

/// What is due now for `hook`: each plugin, library, and position.
async fn due_for(db: &Database, hook: Hook, now: i64) -> Vec<(String, i64, i64)> {
    let due = run(db, move |conn| all(conn, now)).await;
    due.into_iter().filter(|d| d.hook == hook).map(|d| (d.plugin, d.library, d.position)).collect()
}

async fn due_pairs(db: &Database, now: i64) -> Vec<(String, i64, i64)> {
    due_for(db, Hook::TracksChanged, now).await
}

fn due(hook: Hook, failures: i64) -> Due {
    Due { hook, plugin: "p".into(), library: 1, position: 0, failures, user: 0 }
}

#[tokio::test]
async fn a_pair_is_due_while_enabled_with_both_grants_and_changes_waiting() {
    let (_temp, db) = database().await;
    subscribed(&db).await;
    assert!(due_pairs(&db, 0).await.is_empty(), "nothing has changed");

    changes(&db, "(1, 'track', 11, 'upsert', 0), (2, 'track', 21, 'upsert', 0)").await;
    assert_eq!(due_pairs(&db, 0).await, [("p".to_owned(), 1, 0)], "only its own library");

    for (unset, restore) in [
        (
            "DELETE FROM plugin_library_grants WHERE permission = 'libraryRead'",
            "INSERT INTO plugin_library_grants VALUES ('p', 1, 'libraryRead')",
        ),
        (
            "DELETE FROM plugin_library_grants WHERE permission = 'tracksChanged'",
            "INSERT INTO plugin_library_grants VALUES ('p', 1, 'tracksChanged')",
        ),
        ("UPDATE plugin_libraries SET enabled = 0", "UPDATE plugin_libraries SET enabled = 1"),
    ] {
        db.write(move |tx| tx.execute_batch(unset)).await.unwrap();
        assert!(due_pairs(&db, 0).await.is_empty(), "{unset}");
        db.write(move |tx| tx.execute_batch(restore)).await.unwrap();
    }

    db.write(|tx| {
        tx.execute_batch("INSERT INTO plugin_cursors (plugin_id, library_id, hook, retry_at) VALUES ('p', 1, 'tracksChanged', 1000)")
    })
    .await
    .unwrap();
    assert!(due_pairs(&db, 999).await.is_empty(), "waiting to retry");
    assert_eq!(due_pairs(&db, 1000).await.len(), 1);

    db.write(|tx| {
        tx.execute_batch(
            "UPDATE plugin_cursors SET position = (SELECT max(seq) FROM library_changes)",
        )
    })
    .await
    .unwrap();
    assert!(due_pairs(&db, 1000).await.is_empty(), "delivered up to the end");
}

#[tokio::test]
async fn a_batch_names_the_tracks_changed_and_removed() {
    let (_temp, db) = database().await;
    changes(&db, "(1, 'track', 11, 'upsert', 0), (1, 'album', 101, 'upsert', 0), (1, 'track', 12, 'delete', 0), (2, 'track', 21, 'upsert', 0)").await;
    let (next, tracks) = run(&db, |conn| batch(conn, 1, 0)).await;
    assert_eq!((tracks.changed, tracks.removed), (vec![11], vec![12]));
    let (after, rest) = run(&db, move |conn| batch(conn, 1, next)).await;
    assert_eq!(after, next, "nothing further in library 1");
    assert!(rest.changed.is_empty() && rest.removed.is_empty());
}

#[tokio::test]
async fn a_batch_is_at_most_a_hundred_changes() {
    let (_temp, db) = database().await;
    db.write(|tx| {
        tx.execute_batch(
            "WITH RECURSIVE n (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 250)
             INSERT INTO library_changes (library_id, entity_type, entity_id, op, at)
             SELECT 1, 'track', 1000 + i, 'upsert', 0 FROM n",
        )
    })
    .await
    .unwrap();
    let (next, first) = run(&db, |conn| batch(conn, 1, 0)).await;
    let (_, second) = run(&db, move |conn| batch(conn, 1, next)).await;
    assert_eq!((first.changed.len(), second.changed.len()), (100, 100));
    assert_eq!((first.changed[0], second.changed[0]), (1001, 1101));
}

async fn cursor(db: &Database) -> (i64, i64, Option<i64>) {
    run(db, |conn| {
        conn.query_row("SELECT position, failures, retry_at FROM plugin_cursors", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
    })
    .await
}

#[tokio::test]
async fn failures_wait_longer_each_time_then_disable_the_plugin_there() {
    let (_temp, db) = database().await;
    subscribed(&db).await;
    let fail = |failures: i64| {
        let db = db.clone();
        async move {
            db.write(move |tx| {
                record(tx, &due(Hook::TracksChanged, failures), 99, false, "LRCLIB answered 503", 0)
            })
            .await
            .unwrap();
        }
    };
    fail(0).await;
    assert_eq!(cursor(&db).await, (0, 1, Some(BACKOFF_MS)), "the position stays");
    fail(1).await;
    assert_eq!(cursor(&db).await, (0, 2, Some(2 * BACKOFF_MS)));
    for failures in 2..MAX_FAILURES {
        fail(failures).await;
    }
    let (enabled, reason): (bool, String) = run(&db, |conn| {
        conn.query_row("SELECT enabled, disabled_reason FROM plugin_libraries", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
    })
    .await;
    assert!(!enabled);
    assert_eq!(reason, "Stopped after failing 5 times in a row: LRCLIB answered 503");

    db.write(|tx| record(tx, &due(Hook::TracksChanged, 5), 99, true, "", 0)).await.unwrap();
    assert_eq!(cursor(&db).await, (99, 0, None), "a success moves on and clears the failures");
}

#[tokio::test]
async fn a_finished_scan_is_delivered_once_with_its_counts_added_up() {
    let (_temp, db) = database().await;
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO plugin_library_grants VALUES ('p', 1, 'scanFinished'), ('p', 1, 'libraryRead');
             INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES ('p', 1, 1);
             INSERT INTO scans (id, library_id, trigger, state, scopes, added, updated, created_at, finished_at)
             VALUES (1, 1, 'watch', 'completed', '[]', 2, 1, 0, 100),
                    (2, 1, 'watch', 'completed', '[]', 3, 0, 0, 200),
                    (3, 1, 'watch', 'running', '[]', 9, 9, 0, NULL),
                    (4, 2, 'watch', 'completed', '[]', 9, 9, 0, 150);",
        )
    })
    .await
    .unwrap();
    assert!(due_for(&db, Hook::ScanFinished, 100).await.is_empty(), "the first finished only now");
    assert_eq!(due_for(&db, Hook::ScanFinished, 201).await, [("p".to_owned(), 1, 0)]);

    let (next, delivered) = run(&db, |conn| event(conn, &due(Hook::ScanFinished, 0), 201)).await;
    let Some(jewelcase_plugins::Event::ScanFinished(totals)) = delivered else {
        panic!("no scan event")
    };
    assert_eq!(
        (next, totals.added, totals.updated),
        (200, 5, 1),
        "both scans, not the running one"
    );
    db.write(move |tx| record(tx, &due(Hook::ScanFinished, 0), next, true, "", 0)).await.unwrap();
    assert!(due_for(&db, Hook::ScanFinished, 1000).await.is_empty(), "each is delivered once");
}

#[tokio::test]
async fn a_schedule_is_due_when_its_interval_has_passed() {
    let (_temp, db) = database().await;
    db.write(|tx| {
        tx.execute_batch(
            r#"UPDATE plugins SET manifest = '{"permissions":[{"permission":"schedule","everyMinutes":60}]}';
               INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES ('p', 1, 1);"#,
        )
    })
    .await
    .unwrap();
    assert!(due_for(&db, Hook::Schedule, 0).await.is_empty(), "not granted");
    db.write(|tx| tx.execute_batch("INSERT INTO plugin_grants VALUES ('p', 'schedule')"))
        .await
        .unwrap();
    assert_eq!(due_for(&db, Hook::Schedule, 0).await.len(), 1, "at once, the first time");

    let hour = 60 * 60_000;
    db.write(move |tx| record(tx, &due(Hook::Schedule, 0), 1000, true, "", 1000)).await.unwrap();
    assert!(due_for(&db, Hook::Schedule, 1000 + hour - 1).await.is_empty());
    assert_eq!(due_for(&db, Hook::Schedule, 1000 + hour).await.len(), 1);
}

/// `p` granted the played hook and what it needs, enabled in library 1, and connected by user
/// 7 at time 100; user 8 has not connected it.
async fn scrobbling(db: &Database) {
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO plugin_grants VALUES ('p', 'played'), ('p', 'listeningActivity');
             INSERT INTO plugin_library_grants VALUES ('p', 1, 'libraryRead');
             INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES ('p', 1, 1);
             INSERT INTO users (id, username, display_name, role, password, created_at, updated_at)
             VALUES (7, 'seven', 'Seven', 'user', '', 0, 0), (8, 'eight', 'Eight', 'user', '', 0, 0);
             INSERT INTO plugin_user_settings VALUES ('p', 7, '{}', 100);",
        )
    })
    .await
    .unwrap();
}

/// User `user` plays track `track` at `at`: reported as it starts, then twice with `end`.
async fn play(db: &Database, id: &'static str, user: i64, track: i64, at: i64, end: &'static str) {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO plays (id, user_id, library_id, track_id, started_at, listen_time_ms, \
                                context, origin, updated_at) \
             VALUES (?1, ?2, 1, ?3, ?4, 0, '{}', 'context', ?4)",
            rusqlite::params![id, user, track, at],
        )?;
        let ended = "UPDATE plays SET ended = ?2, listen_time_ms = 30000 WHERE id = ?1";
        tx.execute(ended, rusqlite::params![id, end])?;
        // Reported again: it ended once.
        tx.execute(ended, rusqlite::params![id, end])
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn plays_reach_a_plugin_only_for_users_who_connected_it_and_only_since() {
    let (_temp, db) = database().await;
    scrobbling(&db).await;
    play(&db, "before", 7, 11, 50, "finished").await;
    assert!(due_for(&db, Hook::Played, 0).await.is_empty(), "played before they connected");

    play(&db, "a", 7, 11, 200, "finished").await;
    play(&db, "b", 8, 12, 210, "finished").await;
    play(&db, "c", 7, 12, 220, "skipped").await;
    let due_now = run(&db, |conn| all(conn, 0)).await;
    let played = due_now.iter().filter(|d| d.hook == Hook::Played);
    assert_eq!(played.map(|d| (d.library, d.user)).collect::<Vec<_>>(), [(1, 7)], "only theirs");

    let (next, delivered) = run(&db, |conn| plays(conn, "p", 1, 7, 0)).await;
    let summary: Vec<_> =
        delivered.iter().map(|p| (p.track.id, p.started_at, p.listen_time_ms, p.end)).collect();
    use jewelcase_plugins::PlayEnd::{Finished, Skipped};
    assert_eq!(summary, [(11, 200, 30_000, Finished), (12, 220, 30_000, Skipped)]);
    assert_eq!(delivered[0].track.artists, ["Aurora Lane"]);

    let seven = Due { user: 7, ..due(Hook::Played, 0) };
    db.write(move |tx| record(tx, &seven, next, true, "", 0)).await.unwrap();
    assert!(due_for(&db, Hook::Played, 0).await.is_empty(), "each play is delivered once");
}

#[tokio::test]
async fn one_users_failures_never_disable_the_plugin() {
    let (_temp, db) = database().await;
    scrobbling(&db).await;
    for failures in 0..MAX_FAILURES + 2 {
        let failing = Due { user: 7, ..due(Hook::Played, failures) };
        db.write(move |tx| record(tx, &failing, 0, false, "Invalid token", 0)).await.unwrap();
    }
    let enabled: bool = run(&db, |conn| {
        conn.query_row("SELECT enabled FROM plugin_libraries", [], |row| row.get(0))
    })
    .await;
    assert!(enabled);
    let retry: i64 = run(&db, |conn| {
        conn.query_row("SELECT retry_at FROM plugin_cursors WHERE user_id = 7", [], |row| {
            row.get(0)
        })
    })
    .await;
    assert_eq!(retry, 60 * 60_000, "retried hourly");
}

/// `p` set up as for [`scrobbling`], with the playing hook granted too.
async fn now_playing(db: &Database) {
    scrobbling(db).await;
    db.write(|tx| tx.execute_batch("INSERT INTO plugin_grants VALUES ('p', 'playing')"))
        .await
        .unwrap();
}

/// User `user` starts track `track`, first reported at `at`, and has not finished it.
async fn start(db: &Database, id: &'static str, user: i64, track: i64, at: i64) {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO plays (id, user_id, library_id, track_id, started_at, listen_time_ms, \
                                context, origin, updated_at) \
             VALUES (?1, ?2, 1, ?3, ?4, 0, '{}', 'context', ?4)",
            rusqlite::params![id, user, track, at],
        )
    })
    .await
    .unwrap();
}

/// Who `playing` is due for at `now`: each library and user.
async fn playing_due(db: &Database, now: i64) -> Vec<(i64, i64)> {
    let due = run(db, move |conn| all(conn, now)).await;
    due.into_iter().filter(|d| d.hook == Hook::Playing).map(|d| (d.library, d.user)).collect()
}

#[tokio::test]
async fn only_the_newest_start_reaches_a_user_who_connected_it() {
    let (_temp, db) = database().await;
    now_playing(&db).await;
    start(&db, "before", 7, 11, 50).await;
    assert!(playing_due(&db, 60).await.is_empty(), "started before they connected");

    start(&db, "a", 7, 11, 200).await;
    start(&db, "b", 7, 12, 210).await;
    start(&db, "c", 8, 12, 220).await;
    assert_eq!(playing_due(&db, 1000).await, [(1, 7)], "only theirs");

    let (next, now) = run(&db, |conn| playing(conn, "p", 1, 7, 0, 1000)).await.unwrap();
    assert_eq!((now.track.id, now.started_at), (12, 210), "the newest, passing over the first");

    let seven = Due { user: 7, ..due(Hook::Playing, 0) };
    db.write(move |tx| record(tx, &seven, next, true, "", 1000)).await.unwrap();
    assert!(playing_due(&db, 1000).await.is_empty(), "each start is delivered once");
}

#[tokio::test]
async fn a_start_that_ended_or_went_stale_is_never_delivered() {
    let (_temp, db) = database().await;
    now_playing(&db).await;
    start(&db, "a", 7, 11, 200).await;
    assert_eq!(playing_due(&db, 200 + FRESH_MS).await.len(), 1);
    assert!(playing_due(&db, 200 + FRESH_MS + 1).await.is_empty(), "a minute is too late");

    let later = 200 + 2 * FRESH_MS;
    start(&db, "b", 7, 12, later).await;
    db.write(|tx| tx.execute_batch("UPDATE plays SET ended = 'skipped' WHERE id = 'b'"))
        .await
        .unwrap();
    assert!(playing_due(&db, later).await.is_empty(), "skipped before it could be announced");
    let gone = Due { user: 7, ..due(Hook::Playing, 0) };
    let (position, nothing) = run(&db, move |conn| event(conn, &gone, later)).await;
    assert!(nothing.is_none() && position == 0, "nothing to deliver, so nothing moves");

    // A device catching up after being offline reports its plays already ended.
    play(&db, "offline", 7, 11, later, "finished").await;
    assert!(playing_due(&db, later).await.is_empty(), "it was never playing as far as we knew");
}

#[tokio::test]
async fn starts_an_hour_old_are_cleared_away() {
    let (_temp, db) = database().await;
    now_playing(&db).await;
    start(&db, "old", 7, 11, 0).await;
    start(&db, "new", 7, 12, 60 * 60_000 + 1).await;
    let kept: Vec<String> = run(&db, |conn| {
        conn.prepare("SELECT play_id FROM play_starts")?.query_map([], |row| row.get(0))?.collect()
    })
    .await;
    assert_eq!(kept, ["new"]);
}

/// `p` granted the searched hook and what it needs, enabled in library 1, and connected by user
/// 7 at time 100, who has shared their searches with it since then; user 8 shares nothing.
async fn search_sharing(db: &Database) {
    scrobbling(db).await;
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO plugin_grants VALUES ('p', 'searched'), ('p', 'searchActivity');
             INSERT INTO plugin_search_sharing VALUES ('p', 7, 100);",
        )
    })
    .await
    .unwrap();
}

/// User `user` settles on search `id` for `query` at `at`, which matched `tracks` tracks.
async fn searched(db: &Database, id: i64, user: i64, query: &'static str, at: i64, tracks: i64) {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO recent_searches (id, user_id, library_id, query, folded, searched_at, \
                                          found_tracks, selected_type, selected_id) \
             VALUES (?1, ?2, 1, ?3, ?3, ?4, ?5, 'track', 11)",
            rusqlite::params![id, user, query, at, tracks],
        )?;
        tx.execute(
            "INSERT INTO search_events (user_id, library_id, kind, search_id, at) \
             VALUES (?1, 1, 'searched', ?2, ?3)",
            rusqlite::params![user, id, at],
        )
    })
    .await
    .unwrap();
}

/// User 7 removes search `id` at `at`.
async fn forgot(db: &Database, id: i64, at: i64) {
    db.write(move |tx| {
        tx.execute("DELETE FROM recent_searches WHERE id = ?1", [id])?;
        tx.execute(
            "INSERT INTO search_events (user_id, library_id, kind, search_id, at) \
             VALUES (7, 1, 'forgotten', ?1, ?2)",
            rusqlite::params![id, at],
        )
    })
    .await
    .unwrap();
}

/// Who `searched` is due for now: each library and user.
async fn searched_due(db: &Database) -> Vec<(i64, i64)> {
    let due = run(db, |conn| all(conn, 0)).await;
    due.into_iter().filter(|d| d.hook == Hook::Searched).map(|d| (d.library, d.user)).collect()
}

#[tokio::test]
async fn searches_reach_a_plugin_only_from_users_who_share_them_and_only_since() {
    let (_temp, db) = database().await;
    search_sharing(&db).await;
    searched(&db, 1, 7, "before", 50, 2).await;
    assert!(searched_due(&db).await.is_empty(), "searched before they shared");

    searched(&db, 2, 7, "a song they do not own", 200, 0).await;
    searched(&db, 3, 8, "not shared", 210, 1).await;
    assert_eq!(searched_due(&db).await, [(1, 7)], "only theirs");

    let (next, event) = run(&db, |conn| searches(conn, "p", 1, 7, 0)).await;
    let Some(jewelcase_plugins::Event::Searched(found)) = event else { panic!("{event:?}") };
    let summary: Vec<_> = found.iter().map(|s| (s.id, s.query.as_str(), s.found.tracks)).collect();
    assert_eq!(summary, [(2, "a song they do not own", 0)], "nothing found is still sent");
    assert_eq!(found[0].selected.map(|s| s.id), Some(11));

    let seven = Due { user: 7, ..due(Hook::Searched, 0) };
    db.write(move |tx| record(tx, &seven, next, true, "", 0)).await.unwrap();
    assert!(searched_due(&db).await.is_empty(), "each search is delivered once");
}

#[tokio::test]
async fn a_removed_search_is_never_sent_and_a_forget_follows_what_it_names() {
    let (_temp, db) = database().await;
    search_sharing(&db).await;
    searched(&db, 2, 7, "kept", 200, 1).await;
    searched(&db, 3, 7, "removed", 210, 1).await;
    forgot(&db, 3, 220).await;

    let (next, event) = run(&db, |conn| searches(conn, "p", 1, 7, 0)).await;
    let Some(jewelcase_plugins::Event::Searched(found)) = event else { panic!("{event:?}") };
    assert_eq!(found.iter().map(|s| s.id).collect::<Vec<_>>(), [2], "removed before it was sent");
    let (_, event) = run(&db, move |conn| searches(conn, "p", 1, 7, next)).await;
    assert!(
        matches!(event, Some(jewelcase_plugins::Event::SearchesForgotten(Some(ref ids))) if ids == &[3]),
        "{event:?}"
    );
}

#[tokio::test]
async fn stopping_asks_the_plugin_to_forget_everything_even_once_disconnected() {
    let (_temp, db) = database().await;
    search_sharing(&db).await;
    searched(&db, 2, 7, "kept", 200, 1).await;
    db.write(|tx| super::super::personal::disconnect(tx, "p", 7)).await.unwrap();
    let shared: i64 = run(&db, |conn| {
        conn.query_row("SELECT count(*) FROM plugin_search_sharing", [], |row| row.get(0))
    })
    .await;
    assert_eq!(shared, 0, "disconnecting stops sharing");

    assert_eq!(searched_due(&db).await, [(1, 7)], "still told, though no longer connected");
    let (_, event) = run(&db, |conn| searches(conn, "p", 1, 7, 0)).await;
    assert!(
        matches!(event, Some(jewelcase_plugins::Event::SearchesForgotten(None))),
        "the search sent before is not sent now, and everything is to be forgotten: {event:?}"
    );
}

#[tokio::test]
async fn sharing_is_turned_on_only_by_a_connected_user_of_a_plugin_allowed_searches() {
    use super::super::personal::{SharingRefused, set_search_sharing};
    let (_temp, db) = database().await;
    scrobbling(&db).await;
    let manifest = serde_json::json!({
        "id": "p", "name": "P", "version": "1", "apiVersion": "0.3",
        "personalSettings": { "properties": { "token": { "type": "string" } } },
        "permissions": [
            { "permission": "libraryRead", "required": true, "reason": "r" },
            { "permission": "searchActivity", "required": true, "reason": "r" },
            { "permission": "searched", "required": true, "reason": "r" }
        ]
    });
    db.write(move |tx| {
        tx.execute("UPDATE plugins SET manifest = ?1", [manifest.to_string()]).map(drop)
    })
    .await
    .unwrap();
    let set = |user: i64, sharing: bool| {
        let db = db.clone();
        async move {
            db.write(move |tx| set_search_sharing(tx, "p", user, true, sharing)).await.unwrap()
        }
    };
    assert!(matches!(set(8, true).await, Err(SharingRefused::Unconnected)));
    assert!(
        matches!(set(7, true).await, Err(SharingRefused::SearchesNotAllowed)),
        "not granted searches"
    );
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO plugin_grants VALUES ('p', 'searched'), ('p', 'searchActivity')",
        )
    })
    .await
    .unwrap();
    assert!(set(7, true).await.is_ok());
    assert!(set(7, true).await.is_ok(), "turning it on twice keeps when it was first turned on");
    assert!(set(7, false).await.is_ok());
    let stopped: i64 = run(&db, |conn| {
        conn.query_row("SELECT count(*) FROM search_events WHERE kind = 'stopped'", [], |row| {
            row.get(0)
        })
    })
    .await;
    assert_eq!(stopped, 1, "one request to forget, in the one library it serves");
}

/// The whole path with the real lyrics plugin: a change in the feed reaches its `handle` as
/// `tracks-changed`. It is granted everything but the network it requires, so it reports that
/// rather than reaching lrclib.net, and the failure is recorded to retry.
#[tokio::test]
#[ignore = "needs plugins/lrclib-lyrics/build.sh"]
async fn delivers_changes_to_a_real_plugin() {
    let (temp, db) = database().await;
    db.write(|tx| tx.execute_batch("DELETE FROM plugins")).await.unwrap();
    let plugins = Arc::new(Plugins::new(db.clone(), temp.path().join("plugins"), HashMap::new()));
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/lrclib-lyrics/target/lrclib-lyrics.wasm"
    );
    let id = plugins.install(std::fs::read(file).unwrap(), None).await.unwrap();
    let granted = vec![Permission::LibraryRead, Permission::TracksChanged];
    plugins.set_grants(id.clone(), Vec::new(), vec![(1, granted)]).await.unwrap();
    // The network is required, so enabling is refused; enable it as the database allows.
    db.write(|tx| tx.execute_batch("INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES ('lrclib-lyrics', 1, 1)")).await.unwrap();
    changes(&db, "(1, 'track', 11, 'upsert', 0)").await;

    plugins.dispatch().await;
    for _ in 0..200 {
        let summary: Option<String> = run(&db, |conn| {
            conn.query_row("SELECT last_run_summary FROM plugin_libraries", [], |row| row.get(0))
        })
        .await;
        if let Some(summary) = summary {
            assert_eq!(
                summary,
                "the plugin reported an error: needs permission to use the network"
            );
            assert_eq!(cursor(&db).await.1, 1, "one failure, to retry");
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the change was not delivered");
}

/// The whole path with the real ListenBrainz plugin and the real service: a token it does not
/// accept is refused as it is entered, and a play reaches the plugin, which ListenBrainz turns
/// away, so it is kept to send again without disabling the plugin.
#[tokio::test]
#[ignore = "needs plugins/listenbrainz/build.sh and the network"]
async fn delivers_plays_to_a_real_scrobbler() {
    let (temp, db) = database().await;
    db.write(|tx| tx.execute_batch("DELETE FROM plugins")).await.unwrap();
    let plugins = Arc::new(Plugins::new(db.clone(), temp.path().join("plugins"), HashMap::new()));
    let file =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/listenbrainz/target/listenbrainz.wasm");
    let id = plugins.install(std::fs::read(file).unwrap(), None).await.unwrap();
    let plugin_wide = vec![Permission::ListeningActivity, Permission::Network, Permission::Played];
    let per_library = vec![(1, vec![Permission::LibraryRead])];
    plugins.set_grants(id.clone(), plugin_wide, per_library).await.unwrap();
    plugins.set_enabled(id.clone(), 1, true).await.unwrap();
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO users (id, username, display_name, role, password, created_at, updated_at)
             VALUES (7, 'seven', 'Seven', 'user', '', 0, 0);
             INSERT INTO library_access VALUES (7, 1, 0);",
        )
    })
    .await
    .unwrap();

    let token = serde_json::json!({ "token": "not-a-real-token" });
    let refused = plugins.save_personal(id.clone(), 7, false, token.as_object().unwrap().clone());
    match refused.await {
        Err(super::super::PluginError::SettingsInvalid(why)) => {
            assert!(why.contains("ListenBrainz did not accept this token"), "{why}")
        }
        other => panic!("{other:?}"),
    }

    // Connected as if it had been accepted, then a play long enough to count.
    db.write(|tx| {
        tx.execute_batch(
            r#"INSERT INTO plugin_user_settings VALUES ('listenbrainz', 7, '{"token":"not-a-real-token"}', 0)"#,
        )
    })
    .await
    .unwrap();
    play(&db, "a", 7, 11, 1_790_942_400_000, "finished").await;
    db.write(|tx| tx.execute_batch("UPDATE plays SET listen_time_ms = 180000")).await.unwrap();

    plugins.dispatch().await;
    for _ in 0..500 {
        let summary: Option<String> = run(&db, |conn| {
            conn.query_row("SELECT last_run_summary FROM plugin_libraries", [], |row| row.get(0))
        })
        .await;
        if let Some(summary) = summary {
            assert!(summary.contains("ListenBrainz answered 401"), "{summary}");
            let (failures, user): (i64, i64) = run(&db, |conn| {
                conn.query_row("SELECT failures, user_id FROM plugin_cursors", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
            })
            .await;
            assert_eq!((failures, user), (1, 7), "kept for user 7 to send again");
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the play was not delivered");
}
