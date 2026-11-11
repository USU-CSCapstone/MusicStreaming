use rusqlite::types::Value;
use serde_json::json;

use super::*;

const USER: i64 = 7;
const LIBRARY: i64 = 1;

/// A device database holding tracks 1 to 4 of library 1, the user's Liked Songs 50 (track 1), their
/// playlist 10 at version 4 whose items 1, 2 and 3 have adjacent keys, so an item put between them
/// renumbers the playlist, and the track 4 they hid.
fn device() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::create_schema(&conn).unwrap();
    for id in 1..=4 {
        conn.execute(
            "INSERT INTO tracks (id, library_id, album_id, file_size, title, sort_key,
                 artist_sort_key, album_sort_key, codec, container, lossless, sample_rate_hz,
                 channels, duration_us, added_at)
             VALUES (?1, 1, 1, 1, 'T', x'61', x'', x'', 'flac', 'flac', 1, 44100, 2, 1000, 0)",
            [id],
        )
        .unwrap();
    }
    conn.execute_batch(
        "INSERT INTO playlists (id, library_id, user_id, kind, title, folded, sort_key, pinned,
             version, created_at, modified_at, track_count, duration_us)
         VALUES (50, 1, 7, 'liked', 'Liked Songs', 'liked songs', x'6c', 1, 2, 0, 5, 1, 1000),
                (10, 1, 7, 'saved', 'Mix', 'mix', x'6d6978', 0, 4, 0, 6, 3, 3000);
         INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at)
         VALUES (50, 100, 1, 1, 5), (10, 1, 1, 1, 6), (10, 2, 1, 2, 6), (10, 3, 1, 3, 6);
         INSERT INTO hidden_tracks (user_id, library_id, track_id, hidden_at) VALUES (7, 1, 4, 9);",
    )
    .unwrap();
    conn
}

/// Every account row of the database, as text, in order.
fn dump(conn: &Connection) -> Vec<String> {
    let mut rows = Vec::new();
    for table in ["playlists", "playlist_items", "hidden_tracks"] {
        let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY 1, 2")).unwrap();
        let columns = stmt.column_count();
        let found = stmt
            .query_map([], |row| {
                let cells: Vec<Value> = (0..columns).map(|i| row.get(i).unwrap()).collect();
                Ok(format!("{table} {cells:?}"))
            })
            .unwrap();
        rows.extend(found.map(Result::unwrap));
    }
    rows
}

/// `name` applied with `args` at time 100, as a device does.
fn run(conn: &Connection, name: &str, args: serde_json::Value) -> Result<Undo, Error> {
    apply(conn, USER, LIBRARY, name, &args.to_string(), 100)
}

/// Playlist `playlist`'s items as `(key, track ID)`, in key order.
fn items(conn: &Connection, playlist: i64) -> Vec<(i64, i64)> {
    conn.prepare("SELECT key, track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY key")
        .unwrap()
        .query_map([playlist], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Playlist `playlist`'s version.
fn version(conn: &Connection, playlist: i64) -> i64 {
    conn.query_row("SELECT version FROM playlists WHERE id = ?1", [playlist], |r| r.get(0)).unwrap()
}

#[test]
fn revert_restores_exactly_the_rows_before_each_write() {
    let writes = [
        ("like", json!({ "trackId": "2" })),
        ("like", json!({ "trackId": "4" })),
        ("unlike", json!({ "trackId": "1" })),
        ("hide", json!({ "trackId": "1" })),
        ("hide", json!({ "trackId": "2" })),
        ("unhide", json!({ "trackId": "4" })),
        ("createPlaylist", json!({ "id": "77", "title": " New ", "items": ["1", "2", "2"] })),
        ("deletePlaylist", json!({ "playlistId": "10" })),
        ("deletePlaylist", json!({ "playlistId": "50" })),
        ("appendToPlaylist", json!({ "playlistId": "10", "items": ["4", "1"] })),
        ("appendToPlaylist", json!({ "playlistId": "50", "items": ["2", "1"] })),
        (
            "editPlaylist",
            json!({ "playlistId": "10", "baseVersion": 4, "title": "Renamed", "pinned": true,
                "description": "Words" }),
        ),
        (
            "editPlaylist",
            json!({ "playlistId": "10", "baseVersion": 4, "ops": [
                { "op": "add", "items": ["4"], "at": 1 }, { "op": "remove", "positions": [0] },
                { "op": "move", "positions": [1], "at": 0 } ] }),
        ),
        (
            "editPlaylist",
            json!({ "playlistId": "50", "baseVersion": 2,
                "ops": [{ "op": "add", "items": ["3", "4"] }] }),
        ),
    ];
    for (name, args) in writes {
        let conn = device();
        let before = dump(&conn);
        let undo = run(&conn, name, args.clone()).unwrap();
        assert_ne!(dump(&conn), before, "{name} {args} changed nothing");
        let undo: Undo = serde_json::from_str(&serde_json::to_string(&undo).unwrap()).unwrap();
        revert(&conn, &undo).unwrap();
        assert_eq!(dump(&conn), before, "{name} {args}");
    }
}

#[test]
fn a_like_makes_the_liked_songs_a_device_lacks_under_the_given_id_and_reverts_to_none() {
    let conn = device();
    conn.execute_batch(
        "DELETE FROM playlists WHERE id = 50; DELETE FROM playlist_items WHERE playlist_id = 50;",
    )
    .unwrap();
    let before = dump(&conn);
    assert!(matches!(run(&conn, "like", json!({ "trackId": "2" })), Err(Error::Invalid(_))));
    let taken = run(&conn, "like", json!({ "trackId": "2", "likedId": "10" }));
    assert!(matches!(taken, Err(Error::IdTaken)), "an ID another playlist has");
    assert_eq!(dump(&conn), before, "a refused like writes nothing");
    let undo = run(&conn, "like", json!({ "trackId": "2", "likedId": "88" })).unwrap();
    assert_eq!(items(&conn, 88), [(KEY_GAP, 2)]);
    let found = crate::liked::find(&conn, USER, LIBRARY).unwrap();
    assert_eq!(found, Some((88, 2)), "version 1, then the like");
    revert(&conn, &undo).unwrap();
    assert_eq!(dump(&conn), before);
}

#[test]
fn likes_and_hides_exclude_each_other_and_a_like_goes_first() {
    let conn = device();
    run(&conn, "like", json!({ "trackId": "4" })).unwrap();
    let hidden: i64 =
        conn.query_row("SELECT count(*) FROM hidden_tracks", [], |row| row.get(0)).unwrap();
    assert_eq!(hidden, 0);
    assert_eq!(items(&conn, 50), [(100 - KEY_GAP, 4), (100, 1)]);
    assert_eq!(version(&conn, 50), 3);
    run(&conn, "hide", json!({ "trackId": "1" })).unwrap();
    assert_eq!(items(&conn, 50), [(100 - KEY_GAP, 4)]);
    let totals: (i64, i64) = conn
        .query_row("SELECT track_count, duration_us FROM playlists WHERE id = 50", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(totals, (1, 1000));
    // A track already in it stays where it is.
    run(&conn, "like", json!({ "trackId": "4" })).unwrap();
    assert_eq!(items(&conn, 50), [(100 - KEY_GAP, 4)]);
}

#[test]
fn an_edit_applies_whole_against_its_version_or_not_at_all() {
    let conn = device();
    let before = dump(&conn);
    let stale = json!({ "playlistId": "10", "baseVersion": 3, "title": "X" });
    assert!(matches!(run(&conn, "editPlaylist", stale), Err(Error::VersionConflict(4))));
    let range = json!({ "playlistId": "10", "baseVersion": 4, "title": "X",
        "ops": [{ "op": "remove", "positions": [0] }, { "op": "remove", "positions": [5] }] });
    assert!(matches!(run(&conn, "editPlaylist", range), Err(Error::Invalid(_))));
    for (args, wanted) in [
        (json!({ "playlistId": "99", "baseVersion": 1, "title": "X" }), "not found"),
        (json!({ "playlistId": "10", "baseVersion": 4 }), "must change something"),
        (json!({ "playlistId": "10", "baseVersion": 4, "ops": [] }), "must not be empty"),
        (json!({ "playlistId": "10", "baseVersion": 4, "title": " " }), "something in it"),
        (json!({ "playlistId": "50", "baseVersion": 2, "title": "X" }), "keeps its title"),
    ] {
        let error = run(&conn, "editPlaylist", args).unwrap_err().to_string();
        assert!(error.contains(wanted), "{error}");
    }
    assert_eq!(dump(&conn), before);
}

#[test]
fn an_edit_moves_items_by_position_and_renumbers_when_no_key_is_left_between() {
    let conn = device();
    let ops = json!([{ "op": "add", "items": ["4"], "at": 1 }]);
    run(&conn, "editPlaylist", json!({ "playlistId": "10", "baseVersion": 4, "ops": ops }))
        .unwrap();
    let tracks: Vec<i64> = items(&conn, 10).iter().map(|item| item.1).collect();
    assert_eq!(tracks, [1, 4, 2, 3]);
    assert_eq!(items(&conn, 10)[0].0, KEY_GAP, "renumbered, spaced out again");
    assert_eq!(version(&conn, 10), 5);
    let counted: (i64, i64) = conn
        .query_row("SELECT track_count, duration_us FROM playlists WHERE id = 10", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(counted, (4, 4000));
}

#[test]
fn a_playlist_is_created_under_a_free_id_and_an_append_names_no_version() {
    let conn = device();
    run(&conn, "createPlaylist", json!({ "id": "77", "title": "New", "items": ["3", "1"] }))
        .unwrap();
    assert_eq!(items(&conn, 77), [(KEY_GAP, 3), (2 * KEY_GAP, 1)]);
    assert_eq!(version(&conn, 77), 1);
    for id in ["77", "10"] {
        let taken = run(&conn, "createPlaylist", json!({ "id": id, "title": "Again" }));
        assert!(matches!(taken, Err(Error::IdTaken)));
    }
    run(&conn, "appendToPlaylist", json!({ "playlistId": "77", "items": ["2"] })).unwrap();
    assert_eq!(items(&conn, 77).last(), Some(&(3 * KEY_GAP, 2)));
    assert!(matches!(run(&conn, "bogus", json!({})), Err(Error::Invalid(_))));
}

#[test]
fn a_refusal_names_its_code_and_a_conflict_its_current_version() {
    let conn = device();
    let code = |name: &str, args: serde_json::Value| {
        run(&conn, name, args).unwrap_err().refusal().expect("a refusal")
    };
    let stale = code("editPlaylist", json!({ "playlistId": "10", "baseVersion": 1, "title": "X" }));
    assert_eq!((&stale["code"], &stale["currentVersion"]), (&json!("version_conflict"), &json!(4)));
    let missing = code("deletePlaylist", json!({ "playlistId": "99" }));
    assert_eq!(missing["code"], "not_found");
    assert_eq!(code("createPlaylist", json!({ "id": "10", "title": "A" }))["code"], "id_taken");
    assert_eq!(code("bogus", json!({}))["code"], "invalid");
    assert!(Error::Sql(rusqlite::Error::InvalidQuery).refusal().is_none());
}
