use rusqlite::Transaction;

use super::*;
use crate::api::testing::catalog;
use crate::db::testing::{Album, Artist, Track};

/// An empty index read at names `version`, just finished after taking `took`.
fn built(version: i64, took: Duration) -> Built<Index> {
    Built { index: Arc::default(), version, ready: Instant::now(), took }
}

#[test]
fn rebuilds_are_spaced_by_how_long_the_last_took() {
    // A slow build just finished: the library has changed, but it is too soon.
    assert!(!built(1, Duration::from_secs(60)).due(2));
    // A quick one is rebuilt at once.
    assert!(built(1, Duration::ZERO).due(2));
    // Nothing has changed.
    assert!(!built(1, Duration::ZERO).due(1));
}

/// A write to the catalog.
type Write = fn(&Transaction) -> rusqlite::Result<()>;

/// Library 1's names version.
async fn version(db: &Database) -> i64 {
    db.read(|conn| library_number(conn, 1, "names_version")).await.unwrap()
}

#[tokio::test]
async fn only_what_search_indexes_moves_the_names_version() {
    let (_temp, db, _app) = catalog::app().await;
    let changes: [(&str, Write, bool); 23] = [
        (
            "analysis",
            |tx| tx.execute_batch("UPDATE tracks SET loudness_lufs = -7 WHERE id = 1001"),
            false,
        ),
        (
            "a placeholder",
            |tx| tx.execute_batch("UPDATE images SET placeholder = x'00' WHERE id = 40"),
            false,
        ),
        (
            "a feed row",
            |tx| {
                tx.execute_batch(
                    "INSERT INTO library_changes (library_id, entity_type, entity_id, op, changed_at)
                     VALUES (1, 'album', 101, 'upsert', 0)",
                )
            },
            false,
        ),
        (
            "a track's title rewritten",
            |tx| tx.execute_batch("UPDATE tracks SET title = title"),
            false,
        ),
        (
            "an album's title rewritten",
            |tx| tx.execute_batch("UPDATE albums SET title = title"),
            false,
        ),
        (
            "an artist's name rewritten",
            |tx| tx.execute_batch("UPDATE artists SET name = name"),
            false,
        ),
        (
            "a track renamed",
            |tx| tx.execute_batch("UPDATE tracks SET title = 'Here' WHERE id = 1001"),
            true,
        ),
        (
            "an album renamed",
            |tx| tx.execute_batch("UPDATE albums SET title = 'Here' WHERE id = 101"),
            true,
        ),
        (
            "the unknown album named",
            |tx| tx.execute_batch("UPDATE albums SET title = 'Here' WHERE id = 104"),
            true,
        ),
        (
            "an artist renamed",
            |tx| tx.execute_batch("UPDATE artists SET name = 'Here' WHERE id = 10"),
            true,
        ),
        ("a track added", |tx| Track::new(1007, 1, 101).insert(tx), true),
        ("a track deleted", |tx| tx.execute_batch("DELETE FROM tracks WHERE id = 1007"), true),
        ("an album added", |tx| Album::new(105, 1).insert(tx), true),
        ("an album deleted", |tx| tx.execute_batch("DELETE FROM albums WHERE id = 105"), true),
        ("an artist added", |tx| Artist::new(14, 1, "New").insert(tx), true),
        ("an artist deleted", |tx| tx.execute_batch("DELETE FROM artists WHERE id = 14"), true),
        ("a tag's name rewritten", |tx| tx.execute_batch("UPDATE tags SET name = name"), false),
        (
            "a tag's album and artist counts",
            |tx| {
                tx.execute_batch("UPDATE tags SET album_count = 9, artist_count = 9 WHERE id = 30")
            },
            false,
        ),
        (
            "a tag renamed",
            |tx| tx.execute_batch("UPDATE tags SET name = 'Here' WHERE id = 30"),
            true,
        ),
        (
            "a tag carried by another track, which weighs it",
            |tx| {
                tx.execute_batch(
                    "INSERT INTO track_tags (library_id, track_id, tag_id, tag_name)
                     VALUES (1, 1002, 30, 'Rock')",
                )
            },
            true,
        ),
        (
            "a tag added",
            |tx| {
                tx.execute_batch(
                    "INSERT INTO tags (id, library_id, type, name, name_key, sort_key)
                     VALUES (33, 1, 'genre', 'Jazz', 'jazz', CAST('jazz' AS BLOB))",
                )
            },
            true,
        ),
        ("a tag deleted", |tx| tx.execute_batch("DELETE FROM tags WHERE id = 33"), true),
        (
            "another library's tag",
            |tx| {
                tx.execute_batch(
                    "INSERT INTO tags (id, library_id, type, name, name_key, sort_key)
                     VALUES (50, 2, 'genre', 'Jazz', 'jazz', CAST('jazz' AS BLOB))",
                )
            },
            false,
        ),
    ];
    for (change, write, moves) in changes {
        let before = version(&db).await;
        db.write(write).await.unwrap();
        assert_eq!(version(&db).await != before, moves, "{change}");
    }
}

#[tokio::test]
async fn only_lyrics_move_the_lyrics_version_and_they_move_nothing_else() {
    let (_temp, db, _app) = catalog::app().await;
    let versions = async || {
        db.read(|conn| {
            Ok((
                library_number(conn, 1, "names_version")?,
                library_number(conn, 1, "lyrics_version")?,
            ))
        })
        .await
        .unwrap()
    };
    let changes: [(&str, Write, bool); 7] = [
        (
            "lyrics added",
            |tx| {
                tx.execute_batch(
                    "INSERT INTO track_lyrics (library_id, track_id, plain)
                     VALUES (1, 1001, 'Hi'), (1, 1002, 'Hi')",
                )
            },
            true,
        ),
        (
            "lyrics rewritten as they were",
            |tx| tx.execute_batch("UPDATE track_lyrics SET plain = plain"),
            false,
        ),
        (
            "lyrics changed",
            |tx| tx.execute_batch("UPDATE track_lyrics SET plain = 'Bye' WHERE track_id = 1001"),
            true,
        ),
        (
            "plain lyrics made timed",
            |tx| {
                tx.execute_batch(
                    r#"UPDATE track_lyrics SET plain = NULL,
                           synced = '[{"startMs":0,"text":"Bye"}]' WHERE track_id = 1001"#,
                )
            },
            true,
        ),
        (
            "lyrics removed",
            |tx| tx.execute_batch("DELETE FROM track_lyrics WHERE track_id = 1001"),
            true,
        ),
        (
            "a track with lyrics deleted",
            |tx| tx.execute_batch("DELETE FROM tracks WHERE id = 1002"),
            true,
        ),
        (
            "another library's lyrics",
            |tx| {
                tx.execute_batch(
                    "INSERT INTO track_lyrics (library_id, track_id, plain) VALUES (2, 2001, 'Hi')",
                )
            },
            false,
        ),
    ];
    for (change, write, moves) in changes {
        let (names, lyrics) = versions().await;
        db.write(write).await.unwrap();
        let (names_after, lyrics_after) = versions().await;
        assert_eq!(lyrics_after != lyrics, moves, "{change}");
        if change != "a track with lyrics deleted" {
            assert_eq!(names_after, names, "{change} names nothing");
        }
    }
}

/// Library 1's index before `change`, and after it, once any rebuild it makes due is done.
async fn around(change: &'static str) -> (Arc<Index>, Arc<Index>) {
    let (_temp, db, _app) = catalog::app().await;
    let indexes = Indexes::default();
    let before = indexes.index(&db, 1).await.unwrap();
    db.write(move |tx| tx.execute_batch(change)).await.unwrap();
    let slot = indexes.names.libraries.lock().unwrap()[&1].clone();
    let took = slot.built.read().unwrap().as_ref().unwrap().took;
    tokio::time::sleep(took * REBUILD_SPACING).await;
    // This starts a rebuild if the change calls for one, and the lock waits for it.
    indexes.index(&db, 1).await.unwrap();
    drop(slot.building.lock().await);
    (before, indexes.index(&db, 1).await.unwrap())
}

#[tokio::test]
async fn a_write_that_names_nothing_keeps_the_index() {
    let (before, after) = around(
        "UPDATE tracks SET loudness_lufs = -7, title = title WHERE id = 1001;
         INSERT INTO library_changes (library_id, entity_type, entity_id, op, changed_at)
         VALUES (1, 'track', 1001, 'upsert', 0);",
    )
    .await;
    assert!(Arc::ptr_eq(&before, &after));
}

#[tokio::test]
async fn a_rename_rebuilds_the_index() {
    let (before, after) = around("UPDATE tracks SET title = 'Here' WHERE id = 1001").await;
    assert!(!Arc::ptr_eq(&before, &after));
    let found = after.search("here", &[jewelcase_core::Kind::Track], 1);
    assert_eq!(found[0].ids, [1001]);
}

#[tokio::test]
async fn a_deleted_librarys_index_goes_and_none_is_made_for_it() {
    let (_temp, db, _app) = catalog::app().await;
    let indexes = Indexes::default();
    indexes.index(&db, 2).await.unwrap();
    db.write(|tx| tx.execute_batch("DELETE FROM libraries WHERE id = 2")).await.unwrap();
    indexes.index(&db, 1).await.unwrap();
    assert_eq!(indexes.names.libraries.lock().unwrap().keys().collect::<Vec<_>>(), [&1]);
    let index = indexes.index(&db, 2).await.unwrap();
    assert!(index.search("elsewhere", &jewelcase_core::Kind::ALL, 1).is_empty());
    assert_eq!(indexes.names.libraries.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn every_library_is_indexed_at_startup() {
    let (_temp, db, _app) = catalog::app().await;
    let indexes = Arc::new(Indexes::default());
    indexes.build_all(&db);
    crate::api::testing::eventually("both libraries' names and lyrics are indexed", async || {
        fn built<T>(libraries: &Libraries<T>, id: i64) -> bool {
            let libraries = libraries.libraries.lock().unwrap();
            libraries.get(&id).is_some_and(|slot| slot.built.read().unwrap().is_some())
        }
        let all =
            [1, 2].into_iter().all(|id| built(&indexes.names, id) && built(&indexes.lyrics, id));
        all.then_some(())
    })
    .await;
}

/// Library 1's tracks with lyrics: plain, with a label and a blank line; timed; and plain again,
/// so that a lyric index is read in several batches. Library 2's track has lyrics of its own.
const LYRICS: &str = r#"
    INSERT INTO track_lyrics (library_id, track_id, plain, synced) VALUES
        (1, 1001, '[Verse]
Hello from the other side

I must have called', NULL),
        (1, 1002, NULL, '[{"startMs":0,"text":"Say it ain''t so"},{"startMs":900,"text":"I will not go"}]'),
        (1, 1003, 'Turn the lights off', NULL),
        (1, 1004, 'Other side of the world', NULL),
        (1, 1005, 'So long', NULL),
        (2, 2001, 'Hello from elsewhere', NULL);"#;

/// The tracks `index` finds for `query`, each with its best line's place.
fn lyrics_found(index: &LyricIndex, query: &str) -> Vec<(i64, u16)> {
    index.search(query, |_| true).iter().map(|hit| (hit.track, hit.line)).collect()
}

#[tokio::test]
async fn a_librarys_lyrics_are_indexed_by_their_lines() {
    let (_temp, db, _app) = catalog::app().await;
    db.write(|tx| tx.execute_batch(LYRICS)).await.unwrap();
    let index = Indexes::default().lyrics(&db, 1).await.unwrap();
    assert_eq!(index.tracks(), 5, "read in batches, library 1's only");
    // Places count the label and the blank line, as the stored lyrics' lines do; lines as long
    // tie by track.
    assert_eq!(lyrics_found(&index, "other side"), [(1001, 1), (1004, 0)]);
    assert_eq!(lyrics_found(&index, "called"), [(1001, 3)]);
    assert_eq!(lyrics_found(&index, "aint"), [(1002, 0)], "a timed line");
    assert_eq!(lyrics_found(&index, "so long"), [(1005, 0)]);
    assert_eq!(lyrics_found(&index, "elsewhere"), []);
    assert_eq!(lyrics_found(&index, "verse"), [], "a label is not a line");
}

#[tokio::test]
async fn a_lyric_written_rebuilds_only_the_lyric_index_and_a_rename_only_the_names() {
    let (_temp, db, _app) = catalog::app().await;
    let indexes = Indexes::default();
    db.write(|tx| tx.execute_batch(LYRICS)).await.unwrap();
    let (names, lyrics) =
        (indexes.index(&db, 1).await.unwrap(), indexes.lyrics(&db, 1).await.unwrap());

    /// Both of library 1's indexes once any rebuild `indexes` has due is done.
    async fn settled(indexes: &Indexes, db: &Arc<Database>) -> (Arc<Index>, Arc<LyricIndex>) {
        let names = indexes.names.libraries.lock().unwrap()[&1].clone();
        let lyrics = indexes.lyrics.libraries.lock().unwrap()[&1].clone();
        let took = |took: Duration| took * REBUILD_SPACING;
        let names_took = names.built.read().unwrap().as_ref().unwrap().took;
        let lyrics_took = lyrics.built.read().unwrap().as_ref().unwrap().took;
        tokio::time::sleep(took(names_took.max(lyrics_took))).await;
        // These start any rebuild due, and the locks wait for them.
        indexes.index(db, 1).await.unwrap();
        indexes.lyrics(db, 1).await.unwrap();
        drop(names.building.lock().await);
        drop(lyrics.building.lock().await);
        (indexes.index(db, 1).await.unwrap(), indexes.lyrics(db, 1).await.unwrap())
    }

    db.write(|tx| {
        tx.execute_batch("UPDATE track_lyrics SET plain = 'Goodbye' WHERE track_id = 1005")
    })
    .await
    .unwrap();
    let (names_after, lyrics_after) = settled(&indexes, &db).await;
    assert!(Arc::ptr_eq(&names, &names_after), "the names index kept");
    assert!(!Arc::ptr_eq(&lyrics, &lyrics_after), "the lyric index rebuilt");
    assert_eq!(lyrics_found(&lyrics_after, "goodbye"), [(1005, 0)]);
    assert_eq!(lyrics_found(&lyrics_after, "so long"), []);

    db.write(|tx| tx.execute_batch("UPDATE tracks SET title = 'Here' WHERE id = 1001"))
        .await
        .unwrap();
    let (names_renamed, lyrics_renamed) = settled(&indexes, &db).await;
    assert!(!Arc::ptr_eq(&names_after, &names_renamed), "the names index rebuilt");
    assert!(Arc::ptr_eq(&lyrics_after, &lyrics_renamed), "the lyric index kept");
}

/// Library 1's tags matching `q` in a newly built index, as each section's type and IDs.
async fn tags_found(db: &Arc<Database>, q: &str) -> Vec<(Option<String>, Vec<i64>)> {
    let index = Indexes::default().index(db, 1).await.unwrap();
    let found = index.search(q, &[jewelcase_core::Kind::Tag], 10);
    found.into_iter().map(|section| (section.group, section.ids)).collect()
}

#[tokio::test]
async fn tags_are_indexed_by_type_and_weighed_by_their_tracks() {
    let (_temp, db, _app) = catalog::app().await;
    // Rock (30) and the mood Calm (32) are on track 1001; Pop (31) is on none. Punk Rock goes on
    // two tracks and Alt Rock on one.
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO tags (id, library_id, type, name, name_key, sort_key) VALUES
                 (33, 1, 'genre', 'Punk Rock', 'punk rock', CAST('punk rock' AS BLOB)),
                 (34, 1, 'genre', 'Alt Rock', 'alt rock', CAST('alt rock' AS BLOB));
             INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) VALUES
                 (1, 1002, 33, 'Punk Rock'), (1, 1003, 33, 'Punk Rock'), (1, 1004, 34, 'Alt Rock');",
        )
    })
    .await
    .unwrap();
    let genre = |ids: Vec<i64>| (Some("genre".to_owned()), ids);
    // Rock is exact; of the rest, the one on more tracks first.
    assert_eq!(tags_found(&db, "rock").await, [genre(vec![30, 33, 34])]);
    assert_eq!(tags_found(&db, "calm").await, [(Some("mood".to_owned()), vec![32])]);
    assert_eq!(tags_found(&db, "pop").await, [], "no track carries it");

    // Alt Rock goes on two more tracks, and passes Punk Rock.
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) VALUES
                 (1, 1005, 34, 'Alt Rock'), (1, 1006, 34, 'Alt Rock');",
        )
    })
    .await
    .unwrap();
    assert_eq!(tags_found(&db, "rock").await, [genre(vec![30, 34, 33])]);
}
