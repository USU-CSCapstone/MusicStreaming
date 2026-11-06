//! Hiding a track: Hidden Songs, each user's own, and liking and hiding undoing each other.

use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::testing::socket::{connect_with, next_of, serve};
use super::super::testing::{TOKEN, add_device, add_user, app_with_tracks, call, conn};
use crate::db::testing::access;

/// Hides or unhides track `track` of library 1 as `token`'s account, as `method` says: `PUT` or
/// `DELETE`.
async fn hide(app: &Router, token: &str, method: &str, track: &str) -> StatusCode {
    let uri = format!("/api/v1/libraries/1/tracks/{track}/hidden");
    call(app, token, method, &uri, None).await.0
}

/// The owner's Hidden Songs in library 1 for `query`, as track IDs, and the total.
async fn hidden(app: &Router, token: &str, query: &str) -> (Vec<String>, Value) {
    let uri = format!("/api/v1/libraries/1/hidden-tracks{query}");
    let (status, page) = call(app, token, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    let ids = items.iter().map(|track| track["id"].as_str().unwrap().to_owned()).collect();
    (ids, page["total"].clone())
}

/// Whether track 1 is liked by the owner, as listed.
async fn liked(app: &Router) -> bool {
    let (_, page) = call(app, TOKEN, "GET", "/api/v1/libraries/1/tracks?ids=1", None).await;
    page["items"][0]["liked"].as_bool().unwrap()
}

#[tokio::test]
async fn hidden_songs_lists_what_was_hidden_newest_first() {
    let (_temp, _db, app) = app_with_tracks().await;
    for track in ["1", "2", "1"] {
        assert_eq!(hide(&app, TOKEN, "PUT", track).await, StatusCode::NO_CONTENT);
    }
    assert_eq!(hidden(&app, TOKEN, "").await, (vec!["2".into(), "1".into()], json!(2)));
    assert_eq!(hidden(&app, TOKEN, "?order=asc").await.0, ["1", "2"]);
    let (_, first) =
        call(&app, TOKEN, "GET", "/api/v1/libraries/1/hidden-tracks?limit=1", None).await;
    let cursor = first["nextCursor"].as_str().unwrap();
    let rest = hidden(&app, TOKEN, &format!("?limit=1&cursor={cursor}")).await;
    assert_eq!(rest.0, ["1"]);

    for _ in 0..2 {
        assert_eq!(hide(&app, TOKEN, "DELETE", "2").await, StatusCode::NO_CONTENT);
    }
    assert_eq!(hidden(&app, TOKEN, "").await.0, ["1"]);
}

#[tokio::test]
async fn hiding_is_each_users_own_in_each_library() {
    let (temp, _db, app) = app_with_tracks().await;
    {
        let conn = conn(&temp);
        add_user(&conn, 2, "user");
        add_device(&conn, 2, 2, "user-token");
        access(&conn, 2, 1).unwrap();
    }
    // Track 3 is library 2's, so library 1 has no such track.
    assert_eq!(hide(&app, TOKEN, "PUT", "3").await, StatusCode::NOT_FOUND);
    assert_eq!(hide(&app, TOKEN, "PUT", "1").await, StatusCode::NO_CONTENT);
    assert_eq!(hidden(&app, "user-token", "").await.0, Vec::<String>::new());
    let (_, other) = call(&app, TOKEN, "GET", "/api/v1/libraries/2/hidden-tracks", None).await;
    assert_eq!(other["items"], json!([]));
}

#[tokio::test]
async fn liking_and_hiding_undo_each_other() {
    let (_temp, _db, app) = app_with_tracks().await;
    let like = "/api/v1/libraries/1/tracks/1/like";
    assert_eq!(call(&app, TOKEN, "PUT", like, None).await.0, StatusCode::NO_CONTENT);
    assert!(liked(&app).await);
    hide(&app, TOKEN, "PUT", "1").await;
    assert!(!liked(&app).await, "hiding unlikes it");
    assert_eq!(hidden(&app, TOKEN, "").await.0, ["1"]);
    call(&app, TOKEN, "PUT", like, None).await;
    assert!(liked(&app).await);
    assert_eq!(hidden(&app, TOKEN, "").await.0, Vec::<String>::new(), "liking unhides it");

    // Added to Liked Songs by an edit, as a like is, it is unhidden too.
    hide(&app, TOKEN, "PUT", "2").await;
    let (_, lists) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/playlists", None).await;
    let liked_songs = &lists["items"][0];
    let uri = format!("/api/v1/libraries/1/playlists/{}", liked_songs["id"].as_str().unwrap());
    let add = json!({ "baseVersion": liked_songs["version"], "ops": [
        { "op": "add", "items": [{ "type": "track", "id": "2" }] }
    ] });
    assert_eq!(call(&app, TOKEN, "PATCH", &uri, Some(add)).await.0, StatusCode::OK);
    assert_eq!(hidden(&app, TOKEN, "").await.0, Vec::<String>::new());
}

#[tokio::test]
async fn a_hide_reaches_the_users_devices() {
    let (temp, _db, app) = app_with_tracks().await;
    add_device(&conn(&temp), 1, 2, "phone");
    let address = serve(app.clone()).await;
    let mut phone = connect_with(address, "phone").await;
    hide(&app, TOKEN, "PUT", "1").await;
    let message = next_of(&mut phone, "trackHidden").await;
    assert_eq!(
        message,
        json!({ "type": "trackHidden", "libraryId": "1", "trackId": "1", "hidden": true })
    );
    hide(&app, TOKEN, "DELETE", "1").await;
    assert_eq!(next_of(&mut phone, "trackHidden").await["hidden"], false);
}

/// A hide keeps no missing track, so a purge takes the track's hide with it
/// (`requirements/scanning.md` §8).
#[tokio::test]
async fn a_purged_track_takes_its_hide() {
    let (_temp, db, app) = app_with_tracks().await;
    hide(&app, TOKEN, "PUT", "1").await;
    let left: i64 = db
        .write(|tx| {
            tx.execute("DELETE FROM tracks WHERE id = 1", [])?;
            tx.query_row("SELECT count(*) FROM hidden_tracks", [], |row| row.get(0))
        })
        .await
        .unwrap();
    assert_eq!(left, 0);
}

/// The IDs of the items the list at `uri` shows to the owner, a page of `limit` at a time, and
/// each page's total.
async fn walk(app: &Router, uri: &str, limit: usize) -> (Vec<String>, Vec<i64>) {
    let (mut ids, mut totals, mut cursor) = (Vec::new(), Vec::new(), None::<String>);
    loop {
        let after = cursor.map(|c| format!("&cursor={c}")).unwrap_or_default();
        let separator = if uri.contains('?') { '&' } else { '?' };
        let page_uri = format!("{uri}{separator}limit={limit}{after}");
        let (status, page) = call(app, TOKEN, "GET", &page_uri, None).await;
        assert_eq!(status, StatusCode::OK, "{page_uri}: {page}");
        for item in page["items"].as_array().unwrap() {
            ids.push(item["id"].as_str().unwrap().to_owned());
        }
        totals.push(page["total"].as_i64().unwrap());
        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return (ids, totals),
        }
    }
}

/// The IDs of library 1's tracks a list shows for `query`, and each page's total ([`walk`]).
async fn shown(app: &Router, query: &str, limit: usize) -> (Vec<String>, Vec<i64>) {
    let uri = match query.trim_start_matches('&') {
        "" => "/api/v1/libraries/1/tracks".to_owned(),
        query => format!("/api/v1/libraries/1/tracks?{query}"),
    };
    walk(app, &uri, limit).await
}

/// A hidden track is in no list of tracks, and no list counts it: the library's, an album's,
/// an artist's (read from their credits), a tag's (one alone takes its stored count), and
/// search. Unhidden, it is back.
#[tokio::test]
async fn a_hidden_track_is_left_out_of_every_list_and_count() {
    let (_temp, _db, app) = super::super::testing::catalog::app().await;
    let lists = ["", "&albumId=101", "&artistId=10", "&tagAll=30", "&sort=name&order=desc"];
    let mut before = Vec::new();
    for query in lists {
        before.push(shown(&app, query, 100).await);
    }
    assert!(before.iter().all(|(ids, _)| ids.contains(&"1001".to_owned())));

    hide(&app, TOKEN, "PUT", "1001").await;
    for (query, (ids, totals)) in lists.into_iter().zip(&before) {
        for limit in [1, 100] {
            let (now, counted) = shown(&app, query, limit).await;
            let without: Vec<_> = ids.iter().filter(|id| *id != "1001").cloned().collect();
            assert_eq!(now, without, "{query} by {limit}");
            assert!(counted.iter().all(|total| *total == totals[0] - 1), "{query}: {counted:?}");
        }
    }
    let (_, found) =
        call(&app, TOKEN, "GET", "/api/v1/libraries/1/search?q=come%20together", None).await;
    let tracks = found["sections"].as_array().unwrap().iter().find(|s| s["type"] == "tracks");
    assert_eq!(tracks, None, "{found}");

    hide(&app, TOKEN, "DELETE", "1001").await;
    for (query, listed) in lists.into_iter().zip(&before) {
        assert_eq!(&shown(&app, query, 100).await, listed, "{query}");
    }
}

/// A playlist keeps a hidden track in its place, marked, so a device leaves it out of what it
/// shows while its positions stay the playlist's.
#[tokio::test]
async fn a_playlist_marks_a_hidden_track_in_its_place() {
    let (_temp, _db, app) = app_with_tracks().await;
    let items = json!([{ "type": "track", "id": "1" }, { "type": "track", "id": "2" }]);
    let (_, made) = call(
        &app,
        TOKEN,
        "POST",
        "/api/v1/libraries/1/playlists",
        Some(json!({ "title": "Mix", "items": items })),
    )
    .await;
    hide(&app, TOKEN, "PUT", "1").await;
    let uri = format!("/api/v1/libraries/1/playlists/{}/items", made["id"].as_str().unwrap());
    let (_, page) = call(&app, TOKEN, "GET", &uri, None).await;
    let marks: Vec<_> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| (item["track"]["id"].clone(), item["hidden"].clone()))
        .collect();
    assert_eq!(marks, [(json!("1"), json!(true)), (json!("2"), json!(false))]);
    assert_eq!(page["total"], 2, "its positions are the playlist's");
}

/// An album, artist, or tag every track of which the caller hid is gone with them: from its
/// list, and its list's total, from its own page, from search, and from the tag types' counts.
/// One with a track left stays. Unhidden, they are back.
#[tokio::test]
async fn what_is_made_only_of_hidden_tracks_is_hidden_too() {
    let (_temp, _db, app) = super::super::testing::catalog::app().await;
    let lists = ["albums", "artists", "tags"].map(|list| format!("/api/v1/libraries/1/{list}"));
    let mut before = Vec::new();
    for uri in &lists {
        before.push(walk(&app, uri, 100).await);
    }
    let types =
        |app| async move { call(app, TOKEN, "GET", "/api/v1/libraries/1/tag-types", None).await.1 };
    let all_types = types(&app).await;

    // The unknown album's one track, by the unknown artist, and the one track carrying Rock and
    // Calm. Pop is carried by no track here, so no track hides it.
    for track in ["1006", "1001"] {
        hide(&app, TOKEN, "PUT", track).await;
    }
    let gone = [vec!["104"], vec!["13"], vec!["30", "32"]];
    for ((uri, (ids, totals)), gone) in lists.iter().zip(&before).zip(&gone) {
        let left: Vec<_> = ids.iter().filter(|id| !gone.contains(&id.as_str())).cloned().collect();
        assert_eq!(left.len(), ids.len() - gone.len(), "{uri}: all of {gone:?} were listed");
        for limit in [1, 100] {
            let (now, counted) = walk(&app, uri, limit).await;
            assert_eq!(now, left, "{uri} by {limit}");
            let expected = totals[0] - i64::try_from(gone.len()).unwrap();
            assert!(counted.iter().all(|total| *total == expected), "{uri}: {counted:?}");
        }
        for id in gone {
            let (status, _) = call(&app, TOKEN, "GET", &format!("{uri}/{id}"), None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}/{id}");
        }
    }
    let (status, _) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/albums/101", None).await;
    assert_eq!(status, StatusCode::OK, "Abbey Road has tracks left");
    assert_eq!(types(&app).await, json!([{ "key": "genre", "tagCount": 1 }]), "Pop alone");

    // Every track of Abbey Road, so search finds the album no more.
    for track in ["1002", "1003", "1004"] {
        hide(&app, TOKEN, "PUT", track).await;
    }
    let (_, found) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/search?q=abbey", None).await;
    let albums = found["sections"].as_array().unwrap().iter().find(|s| s["type"] == "albums");
    assert_eq!(albums, None, "{found}");

    for track in ["1001", "1002", "1003", "1004", "1006"] {
        hide(&app, TOKEN, "DELETE", track).await;
    }
    for (uri, listed) in lists.iter().zip(&before) {
        assert_eq!(&walk(&app, uri, 100).await, listed, "{uri}");
    }
    assert_eq!(types(&app).await, all_types);
}

/// The owner's view of `uri` in library 1.
async fn get(app: &Router, uri: &str) -> Value {
    let (status, body) = call(app, TOKEN, "GET", &format!("/api/v1/libraries/1{uri}"), None).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

/// Every count is of what the caller sees: a hidden track leaves its library's, album's, disc's,
/// artists', tags' and playlists' counts and durations, and takes from its album and artist the
/// tags only it brought them, and from a tag the albums and artists only it brought it.
#[tokio::test]
async fn counts_leave_out_hidden_tracks() {
    let (_temp, db, app) = super::super::testing::catalog::app().await;
    // Rock on Kid A's Idioteque too, by Radiohead with The Beatles, beside Come Together.
    db.write(|tx| {
        tx.execute(
            "INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) \
             VALUES (1, 1005, 30, 'Rock')",
            [],
        )
    })
    .await
    .unwrap();
    let (_, made) = call(
        &app,
        TOKEN,
        "POST",
        "/api/v1/libraries/1/playlists",
        Some(json!({ "title": "Mix", "items": [
            { "type": "track", "id": "1001" }, { "type": "track", "id": "1003" },
            { "type": "track", "id": "1001" }
        ] })),
    )
    .await;
    let playlist = format!("/playlists/{}", made["id"].as_str().unwrap());
    let duration = |track: &Value| track["durationUs"].as_i64().unwrap();
    let tracks = get(&app, "/tracks?ids=1001,1003,1004").await;
    let [come_together, her_majesty, bonus] = [0, 1, 2].map(|i| duration(&tracks["items"][i]));
    let pages = ["", "/albums/101", "/artists/10", "/artists/11", "/tags/30", &playlist];
    let mut before = Vec::new();
    for uri in pages {
        before.push(get(&app, uri).await);
    }
    let [library, abbey_road, beatles, radiohead, rock, mix] = before.try_into().unwrap();

    for track in ["1001", "1004"] {
        hide(&app, TOKEN, "PUT", track).await;
    }
    let count = |page: &Value, field: &str| page[field].as_i64().unwrap();
    let now = get(&app, "").await;
    assert_eq!(count(&now, "trackCount"), count(&library, "trackCount") - 2);
    assert_eq!(count(&now, "durationUs"), count(&library, "durationUs") - come_together - bonus);
    assert_eq!(count(&now, "albumCount"), count(&library, "albumCount"));

    let now = get(&app, "/albums/101").await;
    assert_eq!(count(&now, "trackCount"), count(&abbey_road, "trackCount") - 2);
    assert_eq!(count(&now, "durationUs"), count(&abbey_road, "durationUs") - come_together - bonus);
    assert_eq!(
        now["discs"][0]["trackCount"],
        abbey_road["discs"][0]["trackCount"].as_i64().unwrap() - 1
    );
    assert_eq!(now["tags"], json!({}), "Rock and Calm came to it only on Come Together");

    let now = get(&app, "/artists/10").await;
    assert_eq!(count(&now, "trackCount"), count(&beatles, "trackCount") - 2);
    assert_eq!(
        now["tags"]["genre"],
        json!([{ "id": "31", "name": "Pop" }, { "id": "30", "name": "Rock" }])
    );
    assert_eq!(now["tags"].get("mood"), None, "Calm came to them only on Come Together");
    let now = get(&app, "/artists/11").await;
    assert_eq!(count(&now, "appearanceCount"), count(&radiohead, "appearanceCount") - 1, "Bonus");

    let now = get(&app, "/tags/30").await;
    let counts = |tag: &Value| ["trackCount", "albumCount", "artistCount"].map(|f| count(tag, f));
    let [tracks, albums, artists] = counts(&rock);
    assert_eq!(
        counts(&now),
        [tracks - 1, albums - 1, artists],
        "The Beatles keep Rock on Idioteque"
    );

    let now = get(&app, &playlist).await;
    assert_eq!((count(&now, "trackCount"), count(&now, "durationUs")), (1, her_majesty));
    assert_eq!(count(&mix, "trackCount"), 3);
}

/// An album's disc count counts an unnumbered disc as disc 1, as its stored count does: hiding
/// the unnumbered tracks leaves disc 1 while it has tracks, and both together take it away.
#[tokio::test]
async fn an_unnumbered_disc_is_gone_only_with_disc_1() {
    let (_temp, db, app) = super::super::testing::catalog::app().await;
    db.write(|tx| {
        use crate::db::testing::{Album, Track};
        Album { disc_count: 2, track_count: 3, ..Album::new(105, 1) }.insert(tx)?;
        for (track, disc) in [(1101, 0), (1102, 1), (1103, 2)] {
            Track { disc_number: disc, ..Track::new(track, 1, 105) }.insert(tx)?;
        }
        tx.execute_batch(
            "INSERT INTO album_discs (library_id, album_id, disc_number, track_count) \
             VALUES (1, 105, 0, 1), (1, 105, 1, 1), (1, 105, 2, 1)",
        )
    })
    .await
    .unwrap();
    let discs = |album: &Value| {
        let numbers = album["discs"].as_array().unwrap().iter().map(|d| d["number"].clone());
        (album["discCount"].clone(), numbers.collect::<Vec<_>>())
    };
    assert_eq!(
        discs(&get(&app, "/albums/105").await),
        (json!(2), vec![json!(null), json!(1), json!(2)])
    );

    hide(&app, TOKEN, "PUT", "1101").await;
    assert_eq!(discs(&get(&app, "/albums/105").await), (json!(2), vec![json!(1), json!(2)]));
    let (_, listed) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/albums?limit=100", None).await;
    let summary = listed["items"].as_array().unwrap().iter().find(|a| a["id"] == "105").unwrap();
    assert_eq!(summary["discCount"], 2, "listed too");

    hide(&app, TOKEN, "PUT", "1102").await;
    assert_eq!(discs(&get(&app, "/albums/105").await), (json!(1), vec![json!(2)]));
}

/// A filter matches nothing only hidden tracks bring: an album or artist that carries a tag, or
/// credits an artist, through hidden tracks alone is left out of a list filtered by it, and of its
/// total, as it is of search.
#[tokio::test]
async fn filters_leave_out_what_only_hidden_tracks_bring() {
    let (_temp, _db, app) = super::super::testing::catalog::app().await;
    let lists = |list: &str, query: &str| format!("/api/v1/libraries/1/{list}?{query}");
    // Each list, and what it loses once Come Together (1001: Rock and Calm) and Bonus (1004,
    // featuring Radiohead) are hidden. The Beatles keep Pop, which none of these tracks brings.
    let cases = [
        (lists("albums", "tagAll=30"), vec!["101"]),
        (lists("albums", "tagAny=30,31"), vec!["101"]),
        (lists("albums", "tagAll=30&tagAny=32"), vec!["101"]),
        (lists("artists", "tagAll=30"), vec!["10"]),
        (lists("artists", "tagAny=32"), vec!["10"]),
        (lists("artists", "tagAny=30,31"), vec![]),
        (lists("albums", "artistId=11&artistCredit=featured"), vec!["101"]),
        (lists("albums", "artistId=11&artistCredit=any"), vec!["101"]),
        (lists("albums", "artistId=10&artistCredit=any"), vec![]),
    ];
    let mut before = Vec::new();
    for (uri, gone) in &cases {
        let listed = walk(&app, uri, 100).await;
        for id in gone {
            assert!(listed.0.contains(&(*id).to_owned()), "{uri} lists {id}: {listed:?}");
        }
        before.push(listed);
    }
    let found = |query: &'static str| {
        let app = app.clone();
        async move {
            let uri = format!("/api/v1/libraries/1/search?{query}");
            let (_, found) = call(&app, TOKEN, "GET", &uri, None).await;
            let sections = found["sections"].as_array().unwrap().clone();
            sections.iter().map(|s| s["type"].as_str().unwrap().to_owned()).collect::<Vec<_>>()
        }
    };
    assert_eq!(found("q=abbey&tagAll=30").await, ["albums"]);
    assert_eq!(found("q=beatles&tagAny=32&types=artists").await, ["artists"]);

    for track in ["1001", "1004"] {
        hide(&app, TOKEN, "PUT", track).await;
    }
    for ((uri, gone), (ids, totals)) in cases.iter().zip(&before) {
        let left: Vec<String> =
            ids.iter().filter(|id| !gone.contains(&id.as_str())).cloned().collect();
        for limit in [1, 100] {
            let (now, counted) = walk(&app, uri, limit).await;
            assert_eq!(now, left, "{uri} by {limit}");
            let expected = totals[0] - i64::try_from(gone.len()).unwrap();
            assert!(counted.iter().all(|total| *total == expected), "{uri}: {counted:?}");
        }
    }
    assert_eq!(found("q=abbey&tagAll=30").await, Vec::<String>::new());
    assert_eq!(found("q=beatles&tagAny=32&types=artists").await, Vec::<String>::new());
    assert_eq!(found("q=beatles&tagAny=31&types=artists").await, ["artists"], "Pop is left");
}

/// An album hidden whole leaves the library's album count and its owner's, and an artist hidden
/// whole the library's artist count; an artist-filtered list of albums loses it from its stored
/// total too.
#[tokio::test]
async fn an_album_hidden_whole_leaves_every_count() {
    let (_temp, _db, app) = super::super::testing::catalog::app().await;
    let library = get(&app, "").await;
    let radiohead = get(&app, "/artists/11").await;
    let radiohead_albums = "/api/v1/libraries/1/albums?artistId=11";
    let (listed, totals) = walk(&app, radiohead_albums, 100).await;
    assert_eq!((listed, totals[0]), (vec!["101".to_owned(), "102".to_owned()], 2), "Bonus, Kid A");

    // Kid A's one track, by Radiohead, who keep their credit on Bonus; and the unknown album's
    // one track, by the unknown artist, who has nothing else.
    for track in ["1005", "1006"] {
        hide(&app, TOKEN, "PUT", track).await;
    }
    let count = |page: &Value, field: &str| page[field].as_i64().unwrap();
    let now = get(&app, "").await;
    assert_eq!(count(&now, "albumCount"), count(&library, "albumCount") - 2);
    assert_eq!(count(&now, "artistCount"), count(&library, "artistCount") - 1);
    let now = get(&app, "/artists/11").await;
    assert_eq!(count(&now, "albumCount"), count(&radiohead, "albumCount") - 1, "Kid A");
    let (_, artists) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/artists", None).await;
    let listed = artists["items"].as_array().unwrap().iter().find(|a| a["id"] == "11").unwrap();
    assert_eq!(listed["albumCount"], now["albumCount"], "listed as shown");

    for limit in [1, 100] {
        let (listed, totals) = walk(&app, radiohead_albums, limit).await;
        assert_eq!(listed, ["101"], "by {limit}");
        assert!(totals.iter().all(|total| *total == 1), "by {limit}: {totals:?}");
    }
    let (owned, totals) = walk(&app, &format!("{radiohead_albums}&artistCredit=owned"), 1).await;
    assert_eq!((owned.len(), totals), (0, vec![0]));
}

/// Each like and unlike reaches the user's devices as `trackLiked`, by track, however it was
/// made: a like, a hide, or an edit of Liked Songs that removes the track.
#[tokio::test]
async fn a_like_reaches_the_users_devices() {
    let (temp, _db, app) = app_with_tracks().await;
    add_device(&conn(&temp), 1, 2, "phone");
    let address = serve(app.clone()).await;
    let mut phone = connect_with(address, "phone").await;
    let liked = |track: &str, liked: bool| json!({ "type": "trackLiked", "libraryId": "1", "trackId": track, "liked": liked });
    let like = |track: &str| format!("/api/v1/libraries/1/tracks/{track}/like");

    call(&app, TOKEN, "PUT", &like("1"), None).await;
    assert_eq!(next_of(&mut phone, "trackLiked").await, liked("1", true));
    hide(&app, TOKEN, "PUT", "1").await;
    assert_eq!(next_of(&mut phone, "trackLiked").await, liked("1", false), "hiding unlikes");

    call(&app, TOKEN, "PUT", &like("2"), None).await;
    assert_eq!(next_of(&mut phone, "trackLiked").await, liked("2", true));
    let (_, lists) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/playlists", None).await;
    let liked_songs = &lists["items"][0];
    let uri = format!("/api/v1/libraries/1/playlists/{}", liked_songs["id"].as_str().unwrap());
    let remove = json!({ "baseVersion": liked_songs["version"], "ops": [
        { "op": "remove", "positions": [0] }
    ] });
    assert_eq!(call(&app, TOKEN, "PATCH", &uri, Some(remove)).await.0, StatusCode::OK);
    assert_eq!(next_of(&mut phone, "trackLiked").await, liked("2", false), "removed is unliked");
}

/// The tracks of playlist `id` of library 1 as `token`'s account sees its items, in order.
async fn items(app: &Router, token: &str, id: &str) -> Vec<String> {
    let uri = format!("/api/v1/libraries/1/playlists/{id}/items");
    let (status, page) = call(app, token, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    items.iter().map(|item| item["track"]["id"].as_str().unwrap().to_owned()).collect()
}

/// Makes a playlist of `items` in library 1 as `token`'s account, and gives its ID.
async fn made(app: &Router, token: &str, items: Value) -> String {
    let body = json!({ "title": "Mix", "items": items });
    let (status, made) =
        call(app, token, "POST", "/api/v1/libraries/1/playlists", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    made["id"].as_str().unwrap().to_owned()
}

/// An album or playlist added to a playlist stands for the tracks its page shows, so the ones
/// the caller hid stay out, and Liked Songs does not unhide them; a track named on its own is
/// added, hidden or not. Another user's additions keep them.
#[tokio::test]
async fn adding_a_collection_leaves_out_what_the_caller_hid() {
    let (temp, _db, app) = app_with_tracks().await;
    {
        let conn = conn(&temp);
        add_user(&conn, 2, "user");
        add_device(&conn, 2, 2, "user-token");
        access(&conn, 2, 1).unwrap();
    }
    hide(&app, TOKEN, "PUT", "1").await;
    let album = json!({ "type": "album", "id": "101" });

    let mix = made(&app, TOKEN, json!([album])).await;
    assert_eq!(items(&app, TOKEN, &mix).await, ["2"], "made from an album");
    let named = made(&app, TOKEN, json!([{ "type": "track", "id": "1" }, album])).await;
    assert_eq!(items(&app, TOKEN, &named).await, ["1", "2"], "a track named on its own");

    let (_, mix_now) =
        call(&app, TOKEN, "GET", &format!("/api/v1/libraries/1/playlists/{mix}"), None).await;
    let edit = json!({ "baseVersion": mix_now["version"], "ops": [
        { "op": "add", "items": [album, { "type": "playlist", "id": named }] }
    ] });
    let uri = format!("/api/v1/libraries/1/playlists/{mix}");
    assert_eq!(call(&app, TOKEN, "PATCH", &uri, Some(edit)).await.0, StatusCode::OK);
    assert_eq!(items(&app, TOKEN, &mix).await, ["2", "2", "2"], "an album and a playlist added");

    let uri = format!("/api/v1/libraries/1/playlists/{mix}/duplicates");
    let (status, found) = call(&app, TOKEN, "POST", &uri, Some(json!({ "items": [album] }))).await;
    assert_eq!((status, &found["trackIds"]), (StatusCode::OK, &json!(["2"])));

    // Added to Liked Songs by an edit, the album likes what it shows and unhides nothing.
    let (_, lists) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/playlists", None).await;
    let liked_songs = &lists["items"][0];
    let id = liked_songs["id"].as_str().unwrap();
    let add = json!({ "baseVersion": liked_songs["version"], "ops": [
        { "op": "add", "items": [album] }
    ] });
    let uri = format!("/api/v1/libraries/1/playlists/{id}");
    assert_eq!(call(&app, TOKEN, "PATCH", &uri, Some(add)).await.0, StatusCode::OK);
    assert_eq!(items(&app, TOKEN, id).await, ["2"]);
    assert_eq!(hidden(&app, TOKEN, "").await.0, ["1"]);

    let theirs = made(&app, "user-token", json!([album])).await;
    assert_eq!(items(&app, "user-token", &theirs).await, ["1", "2"], "only the owner hid it");
}

/// A lookup by ID, which a device makes for the tracks it queued, says which the caller hid, so
/// the device skips them; a list never holds one, so says nothing.
#[tokio::test]
async fn a_lookup_by_id_marks_what_the_caller_hid() {
    let (temp, _db, app) = app_with_tracks().await;
    {
        let conn = conn(&temp);
        add_user(&conn, 2, "user");
        add_device(&conn, 2, 2, "user-token");
        access(&conn, 2, 1).unwrap();
    }
    hide(&app, TOKEN, "PUT", "1").await;
    let marks = |page: &Value| -> Vec<Value> {
        page["items"].as_array().unwrap().iter().map(|track| track["hidden"].clone()).collect()
    };

    let by_ids = "/api/v1/libraries/1/tracks?ids=1,2";
    let (_, page) = call(&app, TOKEN, "GET", by_ids, None).await;
    assert_eq!(marks(&page), [json!(true), Value::Null], "sent only when hidden");
    let (_, track) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/tracks/1", None).await;
    assert_eq!(track["hidden"], true);
    let (_, track) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/tracks/2", None).await;
    assert_eq!(track.get("hidden"), None);
    let (_, page) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/hidden-tracks", None).await;
    assert_eq!(marks(&page), [json!(true)]);

    let (_, page) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/tracks", None).await;
    assert_eq!(marks(&page), [Value::Null]);
    let (_, page) = call(&app, "user-token", "GET", by_ids, None).await;
    assert_eq!(marks(&page), [Value::Null, Value::Null], "only the owner hid it");
}
