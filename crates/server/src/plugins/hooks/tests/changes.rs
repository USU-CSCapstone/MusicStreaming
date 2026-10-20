//! The change hooks, `tracksChanged`, `albumsChanged`, and `artistsChanged`, and how a delivery
//! that fails is tried again.

use jewelcase_plugins::Permission::{
    AlbumsChanged, ArtistsChanged, LibraryRead, Schedule, TracksChanged,
};
use jewelcase_plugins::{Event, Permission};

use super::super::due::batch;
use super::super::{BACKOFF_MS, Hook, MAX_FAILURES};
use super::{HANDLED, HOUR, Received, changes, deliver, setup};
use crate::plugins::testing::{enable, grant};

/// Each event's hook and library, and the IDs it names as changed and removed. Panics unless
/// each event is the one its hook delivers, since every kind carries the same record.
fn named(received: Vec<Received>) -> Vec<(Hook, i64, Vec<u64>, Vec<u64>)> {
    let named = |r: Received| {
        let (case, changes) = match r.event {
            Some(Event::TracksChanged(changes)) => (Hook::TracksChanged, changes),
            Some(Event::AlbumsChanged(changes)) => (Hook::AlbumsChanged, changes),
            Some(Event::ArtistsChanged(changes)) => (Hook::ArtistsChanged, changes),
            other => panic!("not a change: {other:?}"),
        };
        assert_eq!(case, r.hook, "the event is the one its hook delivers");
        (r.hook, r.library, changes.changed, changes.removed)
    };
    received.into_iter().map(named).collect()
}

#[tokio::test]
async fn changes_reach_a_plugin_only_while_it_is_enabled_with_both_grants() {
    let (_temp, db, plugins) = setup().await;
    grant(&plugins, "plugin", &[], &[LibraryRead, TracksChanged]).await;
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "nothing has changed");

    changes(&db, "(1, 'track', 11, 'upsert', 0), (2, 'track', 21, 'upsert', 0)").await;
    let short: [(&[Permission], bool); 3] =
        [(&[TracksChanged], true), (&[LibraryRead], true), (&[LibraryRead, TracksChanged], false)];
    for (granted, enabled) in short {
        grant(&plugins, "plugin", &[], granted).await;
        enable(&plugins, "plugin", enabled).await;
        assert!(deliver(&db, 0, HANDLED).await.is_empty(), "{granted:?}, enabled: {enabled}");
    }
    enable(&plugins, "plugin", true).await;
    let ours = [(Hook::TracksChanged, 1, vec![11], vec![])];
    assert_eq!(named(deliver(&db, 0, HANDLED).await), ours, "only its own library");
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "each change is delivered once");
}

#[tokio::test]
async fn a_hook_is_due_only_where_it_is_granted_and_a_plugin_wide_one_everywhere() {
    let (_temp, db, plugins) = setup().await;
    plugins.set_enabled("plugin".into(), 2, true).await.unwrap();
    grant(&plugins, "plugin", &[Schedule], &[LibraryRead, TracksChanged]).await;
    changes(&db, "(1, 'track', 11, 'upsert', 0), (2, 'track', 21, 'upsert', 0)").await;
    let mut due: Vec<_> =
        deliver(&db, 0, HANDLED).await.into_iter().map(|r| (r.hook, r.library)).collect();
    due.sort_by_key(|&(hook, library)| (hook.name(), library));
    let expected = [(Hook::Schedule, 1), (Hook::Schedule, 2), (Hook::TracksChanged, 1)];
    assert_eq!(due, expected, "library 1's grants are not library 2's");
}

#[tokio::test]
async fn an_event_names_the_tracks_changed_and_removed() {
    let (_temp, db, plugins) = setup().await;
    grant(&plugins, "plugin", &[], &[LibraryRead, TracksChanged]).await;
    changes(
        &db,
        "(1, 'track', 11, 'upsert', 0), (1, 'album', 101, 'upsert', 0), \
         (1, 'track', 12, 'delete', 0), (2, 'track', 21, 'upsert', 0)",
    )
    .await;
    let tracks = [(Hook::TracksChanged, 1, vec![11], vec![12])];
    assert_eq!(named(deliver(&db, 0, HANDLED).await), tracks);
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "nothing further in library 1");
}

/// A batch read past the last change holds nothing to deliver, and leaves the position where it
/// was.
#[tokio::test]
async fn past_the_last_change_there_is_no_batch() {
    let (_temp, db, _plugins) = setup().await;
    changes(&db, "(1, 'track', 11, 'upsert', 0)").await;
    let (position, found) = db.read(|conn| batch(conn, 1, 1_000, "track")).await.unwrap();
    assert_eq!(position, 1_000);
    assert!(found.is_none(), "{found:?}");
}

#[tokio::test]
async fn an_event_names_at_most_a_hundred_changes() {
    let (_temp, db, plugins) = setup().await;
    grant(&plugins, "plugin", &[], &[LibraryRead, TracksChanged]).await;
    db.write(|tx| {
        tx.execute_batch(
            "WITH RECURSIVE n (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 250)
             INSERT INTO library_changes (library_id, entity_type, entity_id, op, changed_at)
             SELECT 1, 'track', 1000 + i, 'upsert', 0 FROM n",
        )
    })
    .await
    .unwrap();
    let mut events = Vec::new();
    for _ in 0..3 {
        events.extend(named(deliver(&db, 0, HANDLED).await));
    }
    let sizes: Vec<_> = events.iter().map(|(.., changed, _)| (changed.len(), changed[0])).collect();
    assert_eq!(sizes, [(100, 1001), (100, 1101), (50, 1201)]);
}

#[tokio::test]
async fn album_and_artist_hooks_hear_only_their_own_kind_in_their_own_library() {
    let (_temp, db, plugins) = setup().await;
    grant(&plugins, "plugin", &[], &[LibraryRead, AlbumsChanged]).await;
    changes(&db, "(1, 'track', 11, 'upsert', 0)").await;
    let nothing = deliver(&db, 0, HANDLED).await;
    assert!(nothing.is_empty(), "a track is not an album, and tracksChanged was not granted");

    changes(
        &db,
        "(1, 'album', 101, 'upsert', 0), (1, 'artist', 301, 'upsert', 0), \
         (1, 'album', 102, 'delete', 0), (2, 'album', 201, 'upsert', 0), \
         (2, 'artist', 401, 'upsert', 0)",
    )
    .await;
    let albums = [(Hook::AlbumsChanged, 1, vec![101], vec![102])];
    assert_eq!(named(deliver(&db, 0, HANDLED).await), albums, "not artists, nor library 2's");
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "each change is delivered once");

    grant(&plugins, "plugin", &[], &[LibraryRead, AlbumsChanged, ArtistsChanged]).await;
    let artists = [(Hook::ArtistsChanged, 1, vec![301], vec![])];
    assert_eq!(named(deliver(&db, 0, HANDLED).await), artists, "its own position, from zero");
}

#[tokio::test]
async fn failures_wait_longer_each_time_then_disable_the_plugin_there() {
    const FAILED: Result<(), &str> = Err("LRCLIB answered 503");
    let (_temp, db, plugins) = setup().await;
    grant(&plugins, "plugin", &[], &[LibraryRead, TracksChanged]).await;
    changes(&db, "(1, 'track', 11, 'upsert', 0)").await;
    let track = |id| vec![(Hook::TracksChanged, 1, vec![id], vec![])];

    assert_eq!(named(deliver(&db, 0, FAILED).await), track(11));
    assert!(deliver(&db, BACKOFF_MS - 1, FAILED).await.is_empty(), "waiting to retry");
    assert_eq!(named(deliver(&db, BACKOFF_MS, FAILED).await), track(11), "from the same place");
    let third = 3 * BACKOFF_MS;
    assert!(deliver(&db, third - 1, FAILED).await.is_empty(), "waiting twice as long");
    assert_eq!(named(deliver(&db, third, FAILED).await), track(11));
    for failure in 4..=MAX_FAILURES {
        assert_eq!(named(deliver(&db, failure * HOUR, FAILED).await), track(11), "{failure}");
    }
    assert!(deliver(&db, 6 * HOUR, HANDLED).await.is_empty(), "disabled");
    let (enabled, reason): (bool, String) = db
        .read(|conn| {
            conn.query_row("SELECT enabled, disabled_reason FROM plugin_libraries", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
        })
        .await
        .unwrap();
    assert!(!enabled);
    assert_eq!(reason, "Stopped after failing 5 times in a row: LRCLIB answered 503");

    enable(&plugins, "plugin", true).await;
    let later = 7 * HOUR;
    assert_eq!(named(deliver(&db, later, HANDLED).await), track(11), "enabled again");
    changes(&db, "(1, 'track', 12, 'upsert', 0)").await;
    assert_eq!(named(deliver(&db, later, FAILED).await), track(12), "moved on");
    let first_wait = later + BACKOFF_MS;
    assert!(deliver(&db, first_wait - 1, FAILED).await.is_empty());
    let again = named(deliver(&db, first_wait, FAILED).await);
    assert_eq!(again, track(12), "a success cleared the failures");
}
