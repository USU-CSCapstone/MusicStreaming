//! Every user has a Liked Songs in each library they reach, and keeps none they deleted.

use rusqlite::{Connection, OptionalExtension};

use super::*;
use crate::db::catalog;
use crate::db::testing::{Album, Image, Track, User, access, library, open};

/// Each Liked Songs as (user, library, title, pinned, track count, artwork), by user and library.
type Row = (i64, i64, String, bool, i64, Option<String>);

/// Every Liked Songs, by user and library.
fn liked(conn: &Connection) -> Result<Vec<Row>> {
    conn.prepare(
        "SELECT user_id, library_id, title, pinned, track_count, image_key FROM playlists \
         WHERE kind = 'liked' ORDER BY user_id, library_id",
    )?
    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
    .collect()
}

/// A new, empty Liked Songs of `user` in `library`.
fn new(user: i64, library: i64) -> Row {
    (user, library, TITLE.into(), true, 0, None)
}

#[tokio::test]
async fn every_user_who_reaches_a_library_has_one_pinned_and_empty() {
    let (_temp, db) = open();
    let (made, granted) = db
        .write(|tx| {
            for (id, role) in [(1, "owner"), (2, "admin"), (3, "user"), (4, "user")] {
                User::new(id, role).insert(tx)?;
            }
            library(tx, 10, "Music", "/music")?;
            let made = liked(tx)?;
            // A grant, which reaches no Liked Songs code yet, is caught by the next ensure.
            access(tx, 3, 10)?;
            ensure(tx)?;
            ensure(tx)?;
            Ok((made, liked(tx)?))
        })
        .await
        .unwrap();
    assert_eq!(made, [new(1, 10), new(2, 10)], "the library's admins have one as it is made");
    assert_eq!(granted, [new(1, 10), new(2, 10), new(3, 10)], "once each; not user 4");
}

#[tokio::test]
async fn one_deleted_is_not_made_again() {
    let (_temp, db) = open();
    let rows = db
        .write(|tx| {
            User::new(1, "owner").insert(tx)?;
            library(tx, 10, "Music", "/music")?;
            tx.execute("DELETE FROM playlists WHERE kind = 'liked'", [])?;
            tx.execute("INSERT INTO liked_songs_deleted (user_id, library_id) VALUES (1, 10)", [])?;
            library(tx, 11, "Other", "/other")?;
            liked(tx)
        })
        .await
        .unwrap();
    assert_eq!(rows, [new(1, 11)], "only in the library it was not deleted in");
}

/// Its artwork is a heart the client draws, so the covers of its tracks never become one, as
/// they do for a saved playlist holding the same track.
#[tokio::test]
async fn it_keeps_no_artwork_of_its_tracks() {
    let (_temp, db) = open();
    let keys = db
        .write(|tx| {
            User::new(1, "owner").insert(tx)?;
            library(tx, 10, "Music", "/music")?;
            Image::new(5, 10, "cover.png").insert(tx)?;
            Album::new(101, 10).insert(tx)?;
            tx.execute("UPDATE albums SET image_id = 5 WHERE id = 101", [])?;
            Track::new(1, 10, 101).insert(tx)?;
            let liked: i64 =
                tx.query_row("SELECT id FROM playlists WHERE kind = 'liked'", [], |r| r.get(0))?;
            tx.execute(
                "INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, \
                 created_at, modified_at) VALUES (20, 10, 1, 'Mine', 'mine', x'', 0, 0)",
                [],
            )?;
            let mut keys = Vec::new();
            for id in [liked, 20] {
                tx.execute(
                    "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
                     VALUES (?1, 1, 10, 1, 0)",
                    [id],
                )?;
                catalog::playlist_artwork(tx, id)?;
                let key = tx
                    .query_row("SELECT image_key FROM playlists WHERE id = ?1", [id], |r| r.get(0))
                    .optional()?;
                keys.push(key.flatten());
            }
            Ok(keys)
        })
        .await
        .unwrap();
    assert_eq!(keys, [None, Some("5".to_owned())]);
}
