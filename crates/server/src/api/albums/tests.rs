use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::testing::{app, catalog, json, page_through};
use super::*;
use crate::api::page;
use crate::db::testing::{Album, library, scattered_key};

#[tokio::test]
async fn pages_through_every_sort_with_unknowns_last() {
    let (_temp, _db, app) = catalog::app().await;
    let base = "/api/v1/libraries/1/albums";
    for limit in [1, 2, 100] {
        assert_eq!(
            page_through(&app, base, limit, "title").await,
            json!(["Abbey Road", "Kid A", "Nevermind", null]),
            "name, limit {limit}"
        );
        assert_eq!(
            page_through(&app, &format!("{base}?order=desc"), limit, "title").await,
            json!(["Nevermind", "Kid A", "Abbey Road", null]),
            "name descending, limit {limit}"
        );
        // Undated albums come last, in name order; there the unknown album's empty name
        // sorts like any other value, since only a sort's first column has an unknown part.
        assert_eq!(
            page_through(&app, &format!("{base}?sort=releaseDate&order=desc"), limit, "title")
                .await,
            json!(["Kid A", "Abbey Road", "Nevermind", null]),
            "newest first, limit {limit}"
        );
        assert_eq!(
            page_through(&app, &format!("{base}?sort=releaseDate"), limit, "title").await,
            json!(["Abbey Road", "Kid A", null, "Nevermind"]),
            "oldest first, limit {limit}"
        );
        assert_eq!(
            page_through(&app, &format!("{base}?sort=dateAdded&order=desc"), limit, "title").await,
            json!(["Kid A", null, "Nevermind", "Abbey Road"]),
            "newest additions first, limit {limit}"
        );
    }
    let (_, page) = json(&app, base).await;
    assert_eq!(page["total"], 4);
}

#[tokio::test]
async fn filters_by_artist_credit() {
    let (_temp, _db, app) = catalog::app().await;
    let base = "/api/v1/libraries/1/albums";
    for (query, expected, total) in [
        ("artistId=11&artistCredit=owned", json!(["Kid A"]), 1),
        ("artistId=11&artistCredit=featured", json!(["Abbey Road"]), 1),
        ("artistId=11", json!(["Abbey Road", "Kid A"]), 2),
        ("artistId=20", json!([]), 0),
        ("artistId=nobody", json!([]), 0),
    ] {
        let (status, page) = json(&app, &format!("{base}?{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        let titles: Vec<Value> =
            page["items"].as_array().unwrap().iter().map(|a| a["title"].clone()).collect();
        assert_eq!(Value::from(titles), expected, "{query}");
        assert_eq!(page["total"], total, "{query}");
    }
}

#[tokio::test]
async fn gets_an_album_with_its_discs() {
    let (_temp, _db, app) = catalog::app().await;
    let (status, album) = json(&app, "/api/v1/libraries/1/albums/101").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        album,
        json!({
            "id": "101", "title": "Abbey Road",
            "artists": [{ "id": "10", "name": "The Beatles" }],
            "releaseDate": "1969-09-26", "type": "album",
            "genres": [{ "id": "30", "name": "Rock" }],
            "trackCount": 17, "trackTotal": 17, "discCount": 1, "discTotal": 1,
            "durationUs": 2_830_000_000_i64, "image": null, "availability": "available",
            "addedAt": "1970-01-01T00:00:01.000Z", "labels": ["Apple"],
            "discs": [{ "number": 1, "trackCount": 17, "trackTotal": 17 }],
        })
    );
    let (_, unknown) = json(&app, "/api/v1/libraries/1/albums/104").await;
    assert_eq!(unknown["title"], Value::Null);
    assert_eq!(unknown["artists"], json!([{ "id": "13", "name": null }]));
    assert_eq!(unknown["trackTotal"], Value::Null);
    assert_eq!(unknown["discs"], json!([{ "number": null, "trackCount": 3, "trackTotal": null }]));
    let (_, missing) = json(&app, "/api/v1/libraries/1/albums/102").await;
    assert_eq!(missing["availability"], "missing");
}

/// Labels stored as anything but strings are an error, never an album with no labels.
#[tokio::test]
async fn labels_that_are_not_strings_are_an_error() {
    let (_temp, db, app) = catalog::app().await;
    db.write(|tx| tx.execute("UPDATE albums SET labels = '[1]' WHERE id = 101", [])).await.unwrap();
    let (status, _) = json(&app, "/api/v1/libraries/1/albums/101").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn another_librarys_album_is_not_found() {
    let (_temp, _db, app) = catalog::app().await;
    for uri in [
        "/api/v1/libraries/1/albums/201",
        "/api/v1/libraries/3/albums/101",
        "/api/v1/libraries/3/albums",
    ] {
        let (status, problem) = json(&app, uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(problem["code"], "not_found", "{uri}");
    }
}

#[tokio::test]
async fn bad_parameters_are_validation_problems() {
    let (_temp, _db, app) = catalog::app().await;
    let base = "/api/v1/libraries/1/albums";
    let (_, page) = json(&app, &format!("{base}?limit=1")).await;
    let name_cursor = page["nextCursor"].as_str().unwrap().to_owned();
    for query in [
        "limit=0".to_owned(),
        "limit=1001".to_owned(),
        "limit=many".to_owned(),
        "sort=playCount".to_owned(),
        "order=sideways".to_owned(),
        "genre=30".to_owned(),
        "cursor=zz".to_owned(),
        format!("sort=dateAdded&cursor={name_cursor}"),
        format!("order=desc&cursor={name_cursor}"),
    ] {
        let (status, problem) = json(&app, &format!("{base}?{query}")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
        assert_eq!(problem["code"], "validation_failed", "{query}");
    }
}

#[tokio::test]
async fn every_sort_reads_from_an_index() {
    let (_temp, db, _app) = app("");
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        // 20,000 albums: 5 per artist, a sixth undated, a few with unknown names.
        for i in 1..=20_000 {
            let artist_key = format!("artist {:05}", i % 4000).into_bytes();
            let album = Album {
                title: Some(format!("Album {i}")),
                sort_key: if i % 1000 == 0 { Vec::new() } else { scattered_key("album", i) },
                artist_sort_key: if i % 997 == 0 { Vec::new() } else { artist_key },
                release_date: (i % 6 != 0).then(|| format!("{:04}", 1950 + i % 75)),
                duration_us: (i % 3000) * 1_000_000,
                added_at: i,
                ..Album::new(i, 1)
            };
            album.insert(tx)?;
        }
        tx.execute_batch("ANALYZE")
    })
    .await
    .unwrap();
    db.read(|conn| {
        for sort in [
            AlbumSort::Name,
            AlbumSort::Artist,
            AlbumSort::DateAdded,
            AlbumSort::ReleaseDate,
            AlbumSort::Duration,
        ] {
            page::assert_indexed(conn, &source(1, None, ArtistCredit::Any), sort.sort())?;
        }
        Ok(())
    })
    .await
    .unwrap();
}
