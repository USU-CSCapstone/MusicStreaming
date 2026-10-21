//! A snapshot's keys: checked to line up with its names, and matching the catalog's artists
//! and albums again after a purge (`design/database.md` §6).

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::snapshot::app;
use super::{report, report_as_owner};

#[tokio::test]
async fn a_purged_artist_and_album_added_again_match_by_key() {
    let (_temp, db, app) = app().await;
    let mut elsewhere = report("2001", 0, Value::Null);
    elsewhere["libraryId"] = json!("2");
    assert_eq!(report_as_owner(&app, json!([elsewhere])).await, StatusCode::NO_CONTENT);
    // The catalog drops an artist and album with their last track, and makes them again, under
    // new IDs, when their music comes back.
    db.write(|tx| {
        tx.execute_batch(
            "DELETE FROM tracks WHERE id = 2001;
             DELETE FROM artists WHERE id = 20;
             DELETE FROM albums WHERE id = 201;
             INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at)
                 VALUES (21, 2, 'Elsewhere', 'elsewhere', x'00', 0);
             INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key,
                                 artist_sort_key, added_at)
                 VALUES (202, 2, 'Elsewhere', 'elsewhere', 'elsewhere', x'00', x'00', 0);",
        )
    })
    .await
    .unwrap();
    let live = db
        .read(|conn| {
            conn.query_row(
                "SELECT artist.id, album.id FROM plays p, json_each(p.artist_keys) credit \
                 JOIN artists artist ON artist.library_id = p.library_id \
                      AND artist.name_key = credit.value \
                 JOIN albums album ON album.library_id = p.library_id \
                      AND album.title_key = p.album_title_key \
                      AND album.artists_key = p.album_artists_key",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
        })
        .await
        .unwrap();
    assert_eq!(live, (21, 202));
}

#[tokio::test]
async fn a_snapshots_arrays_are_checked() {
    let (_temp, db, _app) = app().await;
    let insert = |artists: &'static str, artist_keys: &'static str, genre_keys: &'static str| {
        db.write(move |tx| {
            tx.execute(
                "INSERT INTO plays (id, user_id, library_id, track_id, title, artists, \
                                    artist_keys, album_title_key, album_artists_key, genre_keys, \
                                    duration_us, started_at, listen_time_ms, context, origin, \
                                    updated_at) \
                 VALUES ('p', 1, 1, 1001, 'A', ?1, ?2, '', '', ?3, 0, 0, 0, '{}', 'context', 0)",
                [artists, artist_keys, genre_keys],
            )
        })
    };
    for (artists, artist_keys, genre_keys, broken) in [
        ("{}", "{}", "[]", "artists that are not an array"),
        ("[]", "{}", "[]", "artist keys that are not an array"),
        (r#"["A"]"#, "[]", "[]", "a name without a key"),
        ("[null]", r#"["", "a"]"#, "[]", "a key without a name"),
        ("[]", "[]", "{}", "genres that are not an array"),
    ] {
        let error = insert(artists, artist_keys, genre_keys).await.unwrap_err();
        assert!(error.to_string().contains("CHECK constraint failed"), "{broken}: {error}");
    }
    insert(r#"["A", null]"#, r#"["a", ""]"#, r#"["rock"]"#).await.unwrap();
}
