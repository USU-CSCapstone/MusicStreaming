use axum::http::StatusCode;
use serde_json::json;

use super::super::testing::catalog::{self, page_through};
use super::super::testing::{app, json};
use super::*;
use crate::api::page;
use crate::db::testing::{library, scattered_key};

/// Adds a genre to library 2, named as one of library 1's, on its track.
async fn other_library_genre(db: &crate::db::Database) {
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO tags (id, library_id, type, name, name_key, sort_key)
             VALUES (50, 2, 'genre', 'Rock', 'rock', CAST('rock' AS BLOB));
             INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) VALUES (2, 2001, 50, 'Rock');",
        )
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn lists_the_types_each_library_has_tags_of() {
    let (_temp, db, app) = catalog::app().await;
    let (status, types) = json(&app, "/api/v1/libraries/1/tag-types").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(types, json!([{ "key": "genre", "tagCount": 2 }, { "key": "mood", "tagCount": 1 }]));
    assert_eq!(json(&app, "/api/v1/libraries/2/tag-types").await.1, json!([]));

    other_library_genre(&db).await;
    assert_eq!(
        json(&app, "/api/v1/libraries/2/tag-types").await.1,
        json!([{ "key": "genre", "tagCount": 1 }])
    );
    assert_eq!(json(&app, "/api/v1/libraries/1/tag-types").await.1, types);
    assert_eq!(json(&app, "/api/v1/libraries/999/tag-types").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pages_through_tags_of_one_type_or_every_type() {
    let (_temp, db, app) = catalog::app().await;
    other_library_genre(&db).await;
    let base = "/api/v1/libraries/1/tags";
    for limit in [1, 2, 100] {
        assert_eq!(page_through(&app, base, limit, "name").await, json!(["Calm", "Pop", "Rock"]));
        assert_eq!(
            page_through(&app, &format!("{base}?type=genre&order=desc"), limit, "name").await,
            json!(["Rock", "Pop"]),
            "limit {limit}"
        );
        // Rock and Calm are on a track each, Pop on none; ties go by ID.
        assert_eq!(
            page_through(&app, &format!("{base}?sort=trackCount&order=desc"), limit, "name").await,
            json!(["Calm", "Rock", "Pop"]),
            "limit {limit}"
        );
    }
    let (_, page) = json(&app, &format!("{base}?type=mood")).await;
    assert_eq!(page["total"], 1);
    assert_eq!(
        page["items"][0],
        json!({ "id": "32", "type": "mood", "name": "Calm",
                "trackCount": 1, "albumCount": 1, "artistCount": 1 })
    );
    assert_eq!(json(&app, &format!("{base}?type=colour")).await.1["total"], 0);
    assert_eq!(
        json(&app, &format!("{base}?sort=albumCount")).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn gets_a_tag_of_the_library_and_no_other() {
    let (_temp, db, app) = catalog::app().await;
    other_library_genre(&db).await;
    let (status, rock) = json(&app, "/api/v1/libraries/1/tags/30").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        rock,
        json!({ "id": "30", "type": "genre", "name": "Rock",
                "trackCount": 1, "albumCount": 1, "artistCount": 1 })
    );
    assert_eq!(json(&app, "/api/v1/libraries/2/tags/50").await.0, StatusCode::OK);
    for uri in [
        "/api/v1/libraries/1/tags/50",
        "/api/v1/libraries/2/tags/30",
        "/api/v1/libraries/1/tags/nope",
    ] {
        assert_eq!(json(&app, uri).await.0, StatusCode::NOT_FOUND, "{uri}");
    }
}

/// A list of one type pages through `tags`' indexes after its type, so a deep page costs what the
/// first does.
#[tokio::test]
async fn a_list_of_one_type_reads_from_an_index() {
    let (_temp, db, _app) = app("");
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        // 20,000 tags, split between two types.
        let mut insert = tx.prepare(
            "INSERT INTO tags (id, library_id, type, name, name_key, sort_key, track_count) \
             VALUES (?1, 1, ?2, ?3, ?3, ?4, ?5)",
        )?;
        for i in 1..=20_000_i64 {
            let tag_type = if i % 2 == 0 { "genre" } else { "mood" };
            let name = format!("Tag {i}");
            insert.execute(rusqlite::params![
                i,
                tag_type,
                name,
                scattered_key("tag", i),
                i % 50
            ])?;
        }
        drop(insert);
        tx.execute_batch("ANALYZE")
    })
    .await
    .unwrap();
    db.read(|conn| {
        for sort in [TagSort::Name, TagSort::TrackCount] {
            page::assert_indexed(conn, &source(1, Some("genre".to_owned())), sort.sort())?;
        }
        Ok(())
    })
    .await
    .unwrap();
}
