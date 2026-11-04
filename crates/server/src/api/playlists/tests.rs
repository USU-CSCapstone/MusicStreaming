use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::page;
use super::super::testing::{TOKEN, add_device, add_user, app, app_with_tracks, call, conn};
use super::*;
use crate::db::testing::{
    Artist, Track, access, album_artist, library, scattered_key, track_artist,
};

const URI: &str = "/api/v1/libraries/1/playlists";
const LIBRARY_2: &str = "/api/v1/libraries/2/playlists";

/// Takes away every Liked Songs, which a library's creation gives each admin in it, so a test of
/// saved playlists finds only those it makes. Liked Songs is tested in `liked`.
pub(super) fn saved_only(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM playlists WHERE kind = 'liked'", []).map(drop)
}

/// Library 1 with tracks 1 and 2 on album 101, and album 102 holding tracks 13 (disc 1 number
/// 1), 12 (1, 2), 14 (disc 1, no number) and 11 (disc 2, 1), listed in that album order. Artist
/// 31 is album artist of 102 and features on track 2; artist 41 and library 2's artist 32 are
/// in no playlist. Library 2 holds track 3 on album 201.
async fn catalog() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app_with_tracks().await;
    db.write(|tx| {
        crate::db::testing::Album::new(102, 1).insert(tx)?;
        for (id, disc, number) in
            [(11, 2, Some(1)), (12, 1, Some(2)), (13, 1, Some(1)), (14, 1, None)]
        {
            Track { disc_number: disc, track_number: number, ..Track::new(id, 1, 102) }
                .insert(tx)?;
        }
        Artist::new(31, 1, "Thirty One").insert(tx)?;
        Artist::new(32, 2, "Other").insert(tx)?;
        album_artist(tx, 1, 102, 31)?;
        track_artist(tx, 1, 2, 0, 31, "Thirty One")?;
        saved_only(tx)
    })
    .await
    .unwrap();
    (temp, db, app)
}

/// Creates a playlist as the owner in library 1 and returns its body.
async fn create(app: &Router, body: Value) -> Value {
    let (status, playlist) = call(app, TOKEN, "POST", URI, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{playlist}");
    playlist
}

/// The tracks of playlist `id` in its order, as stored.
async fn tracks(db: &Database, id: &Value) -> Vec<i64> {
    let id: i64 = id.as_str().unwrap().parse().unwrap();
    db.read(move |conn| {
        conn.prepare("SELECT track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY key")?
            .query_map([id], |row| row.get(0))?
            .collect()
    })
    .await
    .unwrap()
}

/// How many playlists are stored, whoever's.
async fn stored(db: &Database) -> i64 {
    db.read(|conn| conn.query_row("SELECT count(*) FROM playlists", [], |row| row.get(0)))
        .await
        .unwrap()
}

/// Adds user 2, who reaches library 1, with the token `user-token`, and an admin, user 3, with
/// `admin-token`, who reaches every library.
fn add_others(temp: &tempfile::TempDir) {
    let conn = conn(temp);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    access(&conn, 2, 1).unwrap();
    add_user(&conn, 3, "admin");
    add_device(&conn, 3, 3, "admin-token");
}

#[tokio::test]
async fn a_playlist_is_created_with_the_tracks_its_items_expand_to_in_order() {
    let (_temp, db, app) = catalog().await;
    let body = json!({
        "title": "  Road  ",
        "description": "Long drives",
        "items": [
            { "type": "track", "id": "2" },
            { "type": "album", "id": "102" },
            { "type": "artist", "id": "31" },
            { "type": "track", "id": "2" },
        ],
    });
    let (status, made) = call(&app, TOKEN, "POST", URI, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().unwrap();
    assert!(made["createdAt"].as_str().unwrap().ends_with('Z'));
    assert_eq!(made["createdAt"], made["modifiedAt"]);
    let mut shown = made.clone();
    for field in ["id", "createdAt", "modifiedAt"] {
        shown.as_object_mut().unwrap().remove(field);
    }
    assert_eq!(
        shown,
        json!({
            "libraryId": "1", "kind": "saved", "title": "Road", "description": "Long drives",
            "image": null, "customImage": false, "trackCount": 11, "durationUs": 11_000_000,
            "pinned": false, "version": 1,
        })
    );
    // An album in its disc and number order; an artist's own album then what it features on, in
    // album order; a track as often as it is named.
    assert_eq!(
        tracks(&db, &made["id"]).await,
        [2, 13, 12, 14, 11, 2, 13, 12, 14, 11, 2].map(i64::from)
    );
    let keys: Vec<i64> = db
        .read(|conn| {
            conn.prepare("SELECT key FROM playlist_items ORDER BY key")?
                .query_map([], |row| row.get(0))?
                .collect()
        })
        .await
        .unwrap();
    assert_eq!(keys[..2], [KEY_GAP, 2 * KEY_GAP]);
    assert!(keys.windows(2).all(|pair| pair[1] - pair[0] == KEY_GAP));

    let (status, got) = call(&app, TOKEN, "GET", &format!("{URI}/{id}"), None).await;
    assert_eq!((status, &got), (StatusCode::OK, &made));
}

#[tokio::test]
async fn a_playlist_can_start_empty_and_from_another_playlist() {
    let (_temp, db, app) = catalog().await;
    let empty = create(&app, json!({ "title": "Empty" })).await;
    assert_eq!(
        (&empty["trackCount"], &empty["durationUs"], &empty["description"], &empty["image"]),
        (&json!(0), &json!(0), &Value::Null, &Value::Null)
    );
    let first =
        create(&app, json!({ "title": "A", "items": [{ "type": "album", "id": "102" }] })).await;
    let copy = create(
        &app,
        json!({ "title": "B", "items": [
            { "type": "playlist", "id": first["id"] }, { "type": "playlist", "id": empty["id"] },
        ] }),
    )
    .await;
    assert_eq!(tracks(&db, &copy["id"]).await, [13, 12, 14, 11]);
}

#[tokio::test]
async fn a_missing_track_is_counted_and_kept_in_place() {
    let (_temp, db, app) = catalog().await;
    db.write(|tx| {
        tx.execute("UPDATE tracks SET missing_since = 5, duration_us = 7 WHERE id = 12", [])
    })
    .await
    .unwrap();
    let made =
        create(&app, json!({ "items": [{ "type": "album", "id": "102" }], "title": "M" })).await;
    assert_eq!((&made["trackCount"], &made["durationUs"]), (&json!(4), &json!(3_000_007)));
    assert_eq!(tracks(&db, &made["id"]).await, [13, 12, 14, 11]);
}

/// An edit's positions count a missing track in its place, so one around it leaves it there,
/// and an add of a playlist that holds one copies it, as create does.
#[tokio::test]
async fn an_edit_counts_a_missing_track_in_its_place() {
    let (_temp, db, app) = catalog().await;
    let made = mixed(&app, &db).await; // [2, 1, 2, 12 (missing), 1]
    let uri = format!("{URI}/{}", made["id"].as_str().unwrap());
    let body = json!({ "baseVersion": 1, "ops": [
        { "op": "remove", "positions": [3] },
        { "op": "move", "positions": [3], "at": 0 },
    ] });
    let (status, edited) = call(&app, TOKEN, "PATCH", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(tracks(&db, &made["id"]).await, [1, 2, 1, 2]);
    assert_eq!(edited["trackCount"], 4);

    let made = mixed(&app, &db).await;
    let uri = format!("{URI}/{}", made["id"].as_str().unwrap());
    let body = json!({ "baseVersion": 1, "ops": [{ "op": "remove", "positions": [4] }] });
    let (status, edited) = call(&app, TOKEN, "PATCH", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(tracks(&db, &made["id"]).await, [2, 1, 2, 12]);
    assert_eq!(edited["trackCount"], 4);

    let copy = create(&app, json!({ "title": "C" })).await;
    let uri = format!("{URI}/{}", copy["id"].as_str().unwrap());
    let body = json!({ "baseVersion": 1, "ops": [{ "op": "add",
        "items": [{ "type": "playlist", "id": made["id"] }] }] });
    let (status, edited) = call(&app, TOKEN, "PATCH", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(tracks(&db, &copy["id"]).await, [2, 1, 2, 12]);
}

#[tokio::test]
async fn a_bad_request_creates_nothing() {
    let (temp, db, app) = catalog().await;
    add_others(&temp);
    let theirs = create_as(&app, "user-token", json!({ "title": "Theirs" })).await;
    let elsewhere = create_in(&app, LIBRARY_2, json!({ "title": "Elsewhere" })).await;
    let bodies = [
        json!({}),
        json!({ "title": "" }),
        json!({ "title": "   " }),
        json!({ "title": 5 }),
        json!({ "title": "T", "items": "track" }),
        json!({ "title": "T", "fromMix": "mix-1" }),
        json!({ "title": "T", "items": [{ "type": "external", "sourceId": "s", "externalId": "e" }] }),
        json!({ "title": "T", "items": [{ "type": "tag", "id": "1" }] }),
        json!({ "title": "T", "items": [{ "id": "1" }] }),
        json!({ "title": "T", "items": [{ "type": "track" }] }),
        json!({ "title": "T", "items": [{ "type": "track", "id": "nope" }] }),
        json!({ "title": "T", "items": [{ "type": "track", "id": "999" }] }),
        // Another library's content and another user's playlist are as absent as nothing.
        json!({ "title": "T", "items": [{ "type": "track", "id": "3" }] }),
        json!({ "title": "T", "items": [{ "type": "album", "id": "201" }] }),
        json!({ "title": "T", "items": [{ "type": "artist", "id": "32" }] }),
        json!({ "title": "T", "items": [{ "type": "playlist", "id": theirs["id"] }] }),
        // The caller's own playlist in another library is as absent as another user's.
        json!({ "title": "T", "items": [{ "type": "playlist", "id": elsewhere["id"] }] }),
        // The good items before a bad one do not survive.
        json!({ "title": "T", "items": [{ "type": "track", "id": "1" }, { "type": "album", "id": "9" }] }),
    ];
    for body in bodies {
        let (status, problem) = call(&app, TOKEN, "POST", URI, Some(body.clone())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}: {problem}");
        assert_eq!(problem["code"], "validation_failed", "{body}");
    }
    assert_eq!(stored(&db).await, 2);
    let (status, _) = call(
        &app,
        TOKEN,
        "POST",
        LIBRARY_2,
        Some(json!({ "title": "T", "items": [{ "type": "track", "id": "1" }] })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "library 1's track in library 2");
    let (status, _) =
        call(&app, TOKEN, "POST", "/api/v1/libraries/9/playlists", Some(json!({ "title": "T" })))
            .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(stored(&db).await, 2);
}

/// Creates a playlist as the owner at `uri`.
async fn create_in(app: &Router, uri: &str, body: Value) -> Value {
    let (status, playlist) = call(app, TOKEN, "POST", uri, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{playlist}");
    playlist
}

/// Creates a playlist as the device with `token` in library 1.
async fn create_as(app: &Router, token: &str, body: Value) -> Value {
    let (status, playlist) = call(app, token, "POST", URI, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{playlist}");
    playlist
}

/// The titles `uri` lists, as the owner, page by page of `limit`, and every page's total.
async fn titles(app: &Router, uri: &str, limit: usize) -> (Vec<String>, Vec<i64>) {
    let (mut titles, mut totals, mut cursor) = (Vec::new(), Vec::new(), None::<String>);
    loop {
        let joiner = if uri.contains('?') { '&' } else { '?' };
        let mut page_uri = format!("{uri}{joiner}limit={limit}");
        if let Some(cursor) = &cursor {
            page_uri += &format!("&cursor={cursor}");
        }
        let (status, page) = call(app, TOKEN, "GET", &page_uri, None).await;
        assert_eq!(status, StatusCode::OK, "{page_uri}: {page}");
        titles.extend(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["title"].as_str().unwrap().to_owned()),
        );
        totals.push(page["total"].as_i64().unwrap());
        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return (titles, totals),
        }
    }
}

/// Four playlists, Delta and Beta pinned, with created and modified times that order them
/// differently from their names.
async fn four(app: &Router, db: &Database) {
    for title in ["Beta", "alpha", "The Cure", "Delta"] {
        create(app, json!({ "title": title })).await;
    }
    db.write(|tx| {
        // created: Delta, The Cure, alpha, Beta; modified: alpha, Beta, Delta, The Cure.
        tx.execute_batch(
            "UPDATE playlists SET
               created_at = CASE title WHEN 'Delta' THEN 1 WHEN 'The Cure' THEN 2 WHEN 'alpha' THEN 3 ELSE 4 END,
               modified_at = CASE title WHEN 'alpha' THEN 1 WHEN 'Beta' THEN 2 WHEN 'Delta' THEN 3 ELSE 4 END,
               pinned = title IN ('Delta', 'Beta')",
        )
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn the_list_puts_pinned_first_then_sorts_in_either_order() {
    let (_temp, db, app) = catalog().await;
    four(&app, &db).await;
    for (query, expected) in [
        ("", ["Beta", "Delta", "alpha", "The Cure"]),
        ("sort=modified&order=asc", ["Beta", "Delta", "alpha", "The Cure"]),
        ("sort=name", ["Beta", "Delta", "alpha", "The Cure"]),
        ("sort=name&order=desc", ["Delta", "Beta", "The Cure", "alpha"]),
        ("sort=created", ["Delta", "Beta", "The Cure", "alpha"]),
        ("sort=created&order=desc", ["Beta", "Delta", "alpha", "The Cure"]),
        ("sort=modified&order=desc", ["Delta", "Beta", "The Cure", "alpha"]),
    ] {
        for limit in [1, 3, 100] {
            let (found, totals) = titles(&app, &format!("{URI}?{query}"), limit).await;
            assert_eq!(found, expected, "{query} by {limit}");
            assert!(totals.iter().all(|total| *total == 4), "{query} by {limit}: {totals:?}");
        }
    }
}

#[tokio::test]
async fn the_list_filters_by_the_words_of_a_title() {
    let (_temp, db, app) = catalog().await;
    four(&app, &db).await;
    create(&app, json!({ "title": "Café Noir" })).await;
    for (q, expected) in [
        ("cure", vec!["The Cure"]),
        ("the cu", vec!["The Cure"]),
        ("ALPHA", vec!["alpha"]),
        ("cafe", vec!["Café Noir"]),
        ("a", vec!["Beta", "Delta", "alpha", "Café Noir"]),
        ("zzz", vec![]),
        ("  ", vec!["Beta", "Delta", "alpha", "Café Noir", "The Cure"]),
    ] {
        let (found, totals) =
            titles(&app, &format!("{URI}?sort=name&q={}", q.replace(' ', "%20")), 2).await;
        assert_eq!(found, expected, "{q}");
        assert!(
            totals.iter().all(|total| *total == i64::try_from(expected.len()).unwrap()),
            "{q}: {totals:?}"
        );
    }
}

#[tokio::test]
async fn the_list_refuses_what_it_does_not_serve() {
    let (_temp, _db, app) = app_with_tracks().await;
    for uri in [
        "/api/v1/libraries/1/playlists?sort=playCount",
        "/api/v1/libraries/1/playlists?sort=size",
        "/api/v1/libraries/1/playlists?limit=0",
        "/api/v1/libraries/1/playlists?pinned=true",
        "/api/v1/libraries/1/playlists?cursor=00",
        // A cursor belongs to the sort and order it came from.
        "/api/v1/libraries/1/playlists?sort=name&cursor=AAA",
    ] {
        let (status, problem) = call(&app, TOKEN, "GET", uri, None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
        assert_eq!(problem["code"], "validation_failed", "{uri}");
    }
    let (status, _) = call(&app, TOKEN, "GET", "/api/v1/libraries/9/playlists", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_cursor_from_one_sort_is_refused_by_another() {
    let (_temp, db, app) = catalog().await;
    four(&app, &db).await;
    let (_, page) = call(&app, TOKEN, "GET", &format!("{URI}?sort=name&limit=1"), None).await;
    let cursor = page["nextCursor"].as_str().unwrap();
    for query in ["sort=created", "sort=name&order=desc"] {
        let (status, _) =
            call(&app, TOKEN, "GET", &format!("{URI}?{query}&cursor={cursor}"), None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
    }
}

#[tokio::test]
async fn every_sort_reads_from_an_index() {
    let (_temp, db, _app) = app("");
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        // 20,000 playlists, a seventh of them pinned.
        for i in 1..=20_000_i64 {
            let title = format!("Playlist {i}");
            tx.execute(
                "INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, pinned, \
                 created_at, modified_at) VALUES (?1, 1, 1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    i,
                    title,
                    title.to_lowercase(),
                    scattered_key("playlist", i),
                    i % 7 == 0,
                    i * 3 % 20_011,
                    i * 5 % 20_011
                ],
            )?;
        }
        tx.execute_batch("ANALYZE")
    })
    .await
    .unwrap();
    db.read(|conn| {
        for sort in [PlaylistSort::Name, PlaylistSort::Created, PlaylistSort::Modified] {
            for q in [None, Some("playlist 1")] {
                page::assert_indexed(conn, &source(1, 1, q), sort.sort())?;
            }
        }
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_playlist_is_its_users_alone() {
    let (temp, db, app) = catalog().await;
    add_others(&temp);
    let made =
        create(&app, json!({ "title": "Mine", "items": [{ "type": "track", "id": "1" }] })).await;
    let id = made["id"].as_str().unwrap();
    let here = format!("{URI}/{id}");
    // Another account, an admin who reaches the library, another library's path, and an ID
    // that is not one.
    for (token, uri) in [
        ("user-token", here.clone()),
        ("admin-token", here.clone()),
        (TOKEN, format!("{LIBRARY_2}/{id}")),
        ("admin-token", format!("{LIBRARY_2}/{id}")),
        (TOKEN, format!("{URI}/{id}0")),
        (TOKEN, format!("{URI}/0{id}")),
        (TOKEN, format!("{URI}/nope")),
    ] {
        for method in ["GET", "DELETE"] {
            let (status, problem) = call(&app, token, method, &uri, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{token} {method} {uri}");
            assert_eq!(problem["code"], "not_found", "{token} {method} {uri}");
        }
    }
    // None of it deleted anything, and no one else's list shows it.
    assert_eq!(stored(&db).await, 1);
    for (token, uris) in [("user-token", vec![URI]), ("admin-token", vec![URI, LIBRARY_2])] {
        for uri in uris {
            let (status, page) = call(&app, token, "GET", uri, None).await;
            assert_eq!(status, StatusCode::OK, "{token} {uri}");
            assert_eq!((&page["items"], &page["total"]), (&json!([]), &json!(0)), "{token} {uri}");
        }
    }
    let (_, own) = call(&app, TOKEN, "GET", LIBRARY_2, None).await;
    assert_eq!(own["total"], 0, "a playlist is in its own library");
    let (_, own) = call(&app, TOKEN, "GET", URI, None).await;
    assert_eq!(own["total"], 1);

    // A user with no access to library 2 is told no more than for a library that is not there.
    let (status, _) =
        call(&app, "user-token", "POST", LIBRARY_2, Some(json!({ "title": "X" }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Their own, with the same ID space, is theirs only.
    let theirs = create_as(&app, "user-token", json!({ "title": "Theirs" })).await;
    let (status, _) =
        call(&app, TOKEN, "GET", &format!("{URI}/{}", theirs["id"].as_str().unwrap()), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) =
        call(&app, "user-token", "GET", &format!("{URI}/{}", theirs["id"].as_str().unwrap()), None)
            .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn deleting_a_playlist_removes_it_and_its_items_and_nothing_else() {
    let (_temp, db, app) = catalog().await;
    let doomed =
        create(&app, json!({ "title": "Doomed", "items": [{ "type": "album", "id": "102" }] }))
            .await;
    let kept =
        create(&app, json!({ "title": "Kept", "items": [{ "type": "track", "id": "1" }] })).await;
    let uri = format!("{URI}/{}", doomed["id"].as_str().unwrap());
    let (status, body) = call(&app, TOKEN, "DELETE", &uri, None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    let (status, _) = call(&app, TOKEN, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&app, TOKEN, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "already gone");
    assert!(tracks(&db, &doomed["id"]).await.is_empty());
    assert_eq!(tracks(&db, &kept["id"]).await, [1]);
    let tracks_left: i64 = db
        .read(|conn| conn.query_row("SELECT count(*) FROM tracks", [], |row| row.get(0)))
        .await
        .unwrap();
    assert_eq!(tracks_left, 7, "the tracks are the library's");
}

/// A link across libraries fails at insert, whatever the code above it does
/// (`design/database.md` §2).
#[tokio::test]
async fn an_item_cannot_name_another_librarys_track() {
    let (_temp, db, app) = catalog().await;
    let made = create(&app, json!({ "title": "Mine" })).await;
    let id: i64 = made["id"].as_str().unwrap().parse().unwrap();
    let across = db
        .write(move |tx| {
            tx.execute(
                "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
                 VALUES (?1, 1, 1, 3, 0)",
                [id],
            )
        })
        .await;
    assert!(across.is_err());
    let elsewhere = db
        .write(move |tx| {
            tx.execute(
                "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
                 VALUES (?1, 1, 2, 3, 0)",
                [id],
            )
        })
        .await;
    assert!(elsewhere.is_err(), "nor a playlist of another library");
}

/// Marks track 12 missing, then creates a playlist of tracks `[2, 1, 2, 12, 1]`: duplicates of
/// tracks 1 and 2 and a missing track, at known positions. Returns the created playlist.
async fn mixed(app: &Router, db: &Database) -> Value {
    db.write(|tx| tx.execute("UPDATE tracks SET missing_since = 5 WHERE id = 12", []))
        .await
        .unwrap();
    let items: Vec<Value> =
        [2, 1, 2, 12, 1].map(|t| json!({ "type": "track", "id": t.to_string() })).into();
    create(app, json!({ "title": "Mixed", "items": items })).await
}

/// The items of a page as `(track id, availability)`.
fn listed(page: &Value) -> Vec<(String, String)> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            let track = &item["track"];
            (track["id"].as_str().unwrap().into(), track["availability"].as_str().unwrap().into())
        })
        .collect()
}

#[tokio::test]
async fn items_page_in_order_with_duplicates_and_missing_tracks_in_place() {
    let (_temp, db, app) = catalog().await;
    let made = mixed(&app, &db).await;
    let uri = format!("{URI}/{}/items", made["id"].as_str().unwrap());
    let (status, all) = call(&app, TOKEN, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{all}");
    let expected: Vec<(String, String)> = [
        ("2", "available"),
        ("1", "available"),
        ("2", "available"),
        ("12", "missing"),
        ("1", "available"),
    ]
    .map(|(id, a)| (id.into(), a.into()))
    .into();
    assert_eq!(listed(&all), expected);
    assert_eq!(
        (&all["total"], &all["version"], &all["nextCursor"]),
        (&json!(5), &json!(1), &Value::Null)
    );
    assert!(all["items"][0]["addedAt"].as_str().unwrap().ends_with('Z'));
    assert!(all["items"][0]["track"]["title"].is_string());

    // Every page size gives the same items in the same order, and the same version.
    for limit in [1, 2, 3, 5] {
        let (mut got, mut cursor) = (Vec::new(), None::<String>);
        loop {
            let after = cursor.map(|c| format!("&cursor={c}")).unwrap_or_default();
            let (status, page) =
                call(&app, TOKEN, "GET", &format!("{uri}?limit={limit}{after}"), None).await;
            assert_eq!(status, StatusCode::OK, "{page}");
            assert_eq!((&page["total"], &page["version"]), (&json!(5), &json!(1)));
            assert!(page["items"].as_array().unwrap().len() <= limit);
            got.extend(listed(&page));
            assert_eq!(page["nextCursor"].is_null(), got.len() == 5, "limit {limit}: {page}");
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(got, expected, "limit {limit}");
    }
}

#[tokio::test]
async fn a_cursor_from_an_older_version_is_gone() {
    let (_temp, db, app) = catalog().await;
    let made = mixed(&app, &db).await;
    let id = made["id"].as_str().unwrap();
    let uri = format!("{URI}/{id}/items");
    let (_, first) = call(&app, TOKEN, "GET", &format!("{uri}?limit=2"), None).await;
    let cursor = first["nextCursor"].as_str().unwrap();
    let id: i64 = id.parse().unwrap();
    db.write(move |tx| tx.execute("UPDATE playlists SET version = 2 WHERE id = ?1", [id]))
        .await
        .unwrap();
    let (status, problem) =
        call(&app, TOKEN, "GET", &format!("{uri}?limit=2&cursor={cursor}"), None).await;
    assert_eq!(status, StatusCode::GONE, "{problem}");
    assert_eq!(problem["code"], "cursor_expired");
    let (status, again) = call(&app, TOKEN, "GET", &format!("{uri}?limit=2"), None).await;
    assert_eq!((status, &again["version"]), (StatusCode::OK, &json!(2)));
    // A made-up cursor and parameters the list does not take are 422.
    for query in ["cursor=nope", "limit=0", "limit=1001", "sort=name", "cursor="] {
        let (status, _) = call(&app, TOKEN, "GET", &format!("{uri}?{query}"), None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
    }
}

#[tokio::test]
async fn duplicates_are_the_present_tracks_once_each_in_request_order() {
    let (_temp, db, app) = catalog().await;
    let made = mixed(&app, &db).await;
    let uri = format!("{URI}/{}/duplicates", made["id"].as_str().unwrap());
    let other =
        create(&app, json!({ "title": "O", "items": [{ "type": "album", "id": "102" }] })).await;
    let body = json!({ "items": [
        { "type": "track", "id": "1" },
        { "type": "track", "id": "13" },
        { "type": "track", "id": "2" },
        { "type": "track", "id": "1" },
        { "type": "album", "id": "101" },
        { "type": "playlist", "id": other["id"] },
        { "type": "track", "id": "12" },
    ] });
    let (status, got) = call(&app, TOKEN, "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    // Album 101 holds 1 and 2; playlist `other` holds 13, 12, 14, 11; 12 is missing but present.
    assert_eq!(got, json!({ "trackIds": ["1", "2", "12"] }));
    let (status, none) = call(
        &app,
        TOKEN,
        "POST",
        &uri,
        Some(json!({ "items": [{ "type": "track", "id": "13" }] })),
    )
    .await;
    assert_eq!((status, none), (StatusCode::OK, json!({ "trackIds": [] })));
    let (status, empty) = call(&app, TOKEN, "POST", &uri, Some(json!({ "items": [] }))).await;
    assert_eq!((status, empty), (StatusCode::OK, json!({ "trackIds": [] })));
    // Not a playable the caller may add, or not a list at all.
    for body in [
        json!({ "items": [{ "type": "track", "id": "3" }] }),
        json!({ "items": [{ "type": "track", "id": "nope" }] }),
        json!({ "items": [{ "type": "external" }] }),
        json!({ "items": "x" }),
        json!({}),
    ] {
        let (status, _) = call(&app, TOKEN, "POST", &uri, Some(body.clone())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
}

#[tokio::test]
async fn another_users_items_and_duplicates_are_not_found() {
    let (temp, db, app) = catalog().await;
    add_others(&temp);
    let made = mixed(&app, &db).await;
    let id = made["id"].as_str().unwrap();
    let body = json!({ "items": [{ "type": "track", "id": "1" }] });
    for (token, base) in
        [("user-token", URI), ("admin-token", URI), (TOKEN, LIBRARY_2), ("admin-token", LIBRARY_2)]
    {
        let (status, problem) = call(&app, token, "GET", &format!("{base}/{id}/items"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{token} {base} items");
        assert_eq!(problem["code"], "not_found");
        let (status, problem) =
            call(&app, token, "POST", &format!("{base}/{id}/duplicates"), Some(body.clone())).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{token} {base} duplicates");
        assert_eq!(problem["code"], "not_found");
    }
    for bad in [format!("{id}0"), format!("0{id}"), "nope".into()] {
        let (status, _) = call(&app, TOKEN, "GET", &format!("{URI}/{bad}/items"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{bad}");
        let (status, _) =
            call(&app, TOKEN, "POST", &format!("{URI}/{bad}/duplicates"), Some(body.clone())).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{bad}");
    }
    // The owner's cursor, used on another user's playlist, reads only that playlist's rows.
    let (_, first) = call(&app, TOKEN, "GET", &format!("{URI}/{id}/items?limit=2"), None).await;
    let cursor = first["nextCursor"].as_str().unwrap();
    let theirs = create_as(&app, "user-token", json!({ "title": "T" })).await;
    let (status, page) = call(
        &app,
        "user-token",
        "GET",
        &format!("{URI}/{}/items?cursor={cursor}", theirs["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!((status, &page["items"], &page["total"]), (StatusCode::OK, &json!([]), &json!(0)));
    // Their own playlist is not a playable for a duplicates check on another user's playlist.
    let (status, _) = call(
        &app,
        "user-token",
        "POST",
        &format!("{URI}/{}/duplicates", theirs["id"].as_str().unwrap()),
        Some(json!({ "items": [{ "type": "playlist", "id": made["id"] }] })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// Both queries search the primary key or `playlist_items_by_track`, never scan, in a playlist
/// of 50,000 items among others.
#[tokio::test]
async fn items_and_duplicates_read_from_an_index() {
    let (_temp, db, _app) = app("");
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        for p in 1..=3 {
            tx.execute(
                "INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, \
                 created_at, modified_at) VALUES (?1, 1, 1, 'A', 'a', x'00', 0, 0)",
                [p],
            )?;
        }
        crate::db::testing::Album::new(101, 1).insert(tx)?;
        for t in 1..=50_000_i64 {
            crate::db::testing::Track::new(t, 1, 101).insert(tx)?;
        }
        tx.execute(
            "INSERT INTO playlist_items SELECT 1 + id % 3, id * 4, 1, id, 0 FROM tracks",
            [],
        )?;
        tx.execute_batch("ANALYZE")
    })
    .await
    .unwrap();
    db.read(|conn| {
        let items = (super::items::ITEMS, ["1", "1000", "101"]);
        let held = (super::items::HELD, ["1", "1", "[3,6,9]"]);
        for (sql, args) in [items, held] {
            let plan: Vec<String> = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
                .query_map(
                    rusqlite::params_from_iter(args.map(|a| {
                        a.parse::<i64>().map_or(
                            rusqlite::types::Value::from(a.to_owned()),
                            rusqlite::types::Value::from,
                        )
                    })),
                    |row| row.get(3),
                )?
                .collect::<Result<_, _>>()?;
            let plan = plan.join(" | ");
            assert!(!plan.contains("SCAN playlist_items"), "{plan}");
            // The duplicates' sort is of the request's own few IDs.
            let index =
                if sql == super::items::HELD { "playlist_items_by_track" } else { "PRIMARY KEY" };
            assert!(plan.contains(index), "{plan}");
            assert!(sql == super::items::HELD || !plan.contains("TEMP B-TREE"), "{plan}");
            assert!(plan.contains("SEARCH"), "{plan}");
        }
        Ok(())
    })
    .await
    .unwrap();
}

mod artwork;
mod edit;
mod liked;
mod realtime;
