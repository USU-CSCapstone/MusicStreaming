//! A playlist's artwork: its ID follows the first four distinct album images, and `getImage`
//! composes the mosaic.

use jewelcase_scanner::describe_image;

use super::super::super::testing::{ffmpeg, send};
use super::*;
use crate::api::Images;
use crate::db::testing::{Image, Track};

const PNGS: [&str; 6] = ["c1.png", "c2.png", "c3.png", "c4.png", "c5.png", "c6.png"];

/// Library 1 holds six covers, `c1.png` to `c6.png` of 40×30, images 1 to 6, each on its own
/// album 101 to 106 holding track 1 to 6; track 7 is a second track of album 101, and track 8 is
/// on album 107, which has no image. Library 2 holds image 20, on album 201 with track 20.
async fn covers() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app("");
    let (music, other) = (temp.path().join("music"), temp.path().join("other"));
    std::fs::create_dir(&music).unwrap();
    std::fs::create_dir(&other).unwrap();
    for (dir, name) in PNGS.iter().map(|name| (&music, name)).chain([(&other, &"c20.png")]) {
        let file = dir.join(name);
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "color=green:s=40x30",
            "-frames:v",
            "1",
            file.to_str().unwrap(),
        ]);
    }
    db.write(move |tx| {
        library(tx, 1, "Music", music)?;
        library(tx, 2, "Other", other)?;
        for (n, name) in (1..).zip(PNGS) {
            let image = Image { width: 40, height: 30, ..Image::new(n, 1, name) };
            image.insert(tx)?;
            crate::db::testing::Album::new(100 + n, 1).insert(tx)?;
            tx.execute("UPDATE albums SET image_id = ?1 WHERE id = ?2", [n, 100 + n])?;
            Track::new(n, 1, 100 + n).insert(tx)?;
        }
        Track::new(7, 1, 101).insert(tx)?;
        crate::db::testing::Album::new(107, 1).insert(tx)?;
        Track::new(8, 1, 107).insert(tx)?;
        Image { width: 40, height: 30, ..Image::new(20, 2, "c20.png") }.insert(tx)?;
        crate::db::testing::Album::new(201, 2).insert(tx)?;
        tx.execute("UPDATE albums SET image_id = 20 WHERE id = 201", [])?;
        Track::new(20, 2, 201).insert(tx)
    })
    .await
    .unwrap();
    (temp, db, app)
}

/// The JSON items that add the tracks `ids`.
fn tracks_of(ids: &[i64]) -> Value {
    json!(ids.iter().map(|id| json!({ "type": "track", "id": id.to_string() })).collect::<Vec<_>>())
}

/// Edits `playlist` at its version with `ops` and returns the edited playlist and its `image` id,
/// `null` or not.
async fn edited(app: &Router, playlist: &Value, ops: Value) -> (Value, Value) {
    let uri = format!("{URI}/{}", playlist["id"].as_str().unwrap());
    let body = json!({ "baseVersion": playlist["version"], "ops": ops });
    let (status, edited) = call(app, TOKEN, "PATCH", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    (edited.clone(), edited["image"]["id"].clone())
}

/// Creates a playlist of the tracks `ids`, in order, and returns it.
async fn created(app: &Router, ids: &[i64]) -> Value {
    create(app, json!({ "title": "P", "items": tracks_of(ids) })).await
}

#[tokio::test]
async fn the_id_follows_the_first_four_distinct_album_images() {
    let (_temp, _db, app) = covers().await;
    // Track 7 shares album 101 with track 1, and track 8's album has no image: neither is a
    // cover of its own.
    let playlist = created(&app, &[1, 7, 8, 2, 3, 4, 5, 6]).await;
    assert_eq!(playlist["image"]["id"], "m1-2-3-4");
    assert_eq!(playlist["customImage"], false);
    assert!(playlist["image"]["placeholder"].is_null());

    // Later tracks, however added or moved, are not among the first four covers.
    let (playlist, id) =
        edited(&app, &playlist, json!([{ "op": "add", "items": tracks_of(&[7, 5]) }])).await;
    assert_eq!(id, "m1-2-3-4");
    let (playlist, id) =
        edited(&app, &playlist, json!([{ "op": "move", "positions": [8], "at": 9 }])).await;
    assert_eq!(id, "m1-2-3-4");
    // A track of an album already among them, put first, adds no cover.
    let (playlist, id) =
        edited(&app, &playlist, json!([{ "op": "add", "items": tracks_of(&[7]), "at": 0 }])).await;
    assert_eq!(id, "m1-2-3-4");
    // A change to the details touches no item.
    let uri = format!("{URI}/{}", playlist["id"].as_str().unwrap());
    let body = json!({ "baseVersion": playlist["version"], "title": "Q" });
    let (_, renamed) = call(&app, TOKEN, "PATCH", &uri, Some(body)).await;
    assert_eq!(renamed["image"]["id"], "m1-2-3-4");

    // A new first cover, a reordered one, and a removed one each change it.
    let (playlist, id) =
        edited(&app, &renamed, json!([{ "op": "add", "items": tracks_of(&[6]), "at": 0 }])).await;
    assert_eq!(id, "m6-1-2-3");
    let (playlist, id) =
        edited(&app, &playlist, json!([{ "op": "move", "positions": [0], "at": 4 }])).await;
    assert_ne!(id, "m6-1-2-3");
    let (playlist, _) =
        edited(&app, &playlist, json!([{ "op": "remove", "positions": [0] }])).await;
    // What is stored is what get and list show.
    let (_, got) =
        call(&app, TOKEN, "GET", &format!("{URI}/{}", playlist["id"].as_str().unwrap()), None)
            .await;
    let (_, listed) = call(&app, TOKEN, "GET", URI, None).await;
    assert_eq!(got["image"], playlist["image"]);
    assert_eq!(listed["items"][0]["image"], playlist["image"]);
}

#[tokio::test]
async fn fewer_than_four_covers_show_the_first_alone_and_none_shows_null() {
    let (_temp, db, app) = covers().await;
    db.write(|tx| tx.execute("UPDATE images SET placeholder = x'0102' WHERE id = 3", []))
        .await
        .unwrap();
    assert!(created(&app, &[]).await["image"].is_null());
    assert!(created(&app, &[8]).await["image"].is_null());
    assert_eq!(created(&app, &[8, 3, 7]).await["image"]["id"], "3");
    assert_eq!(created(&app, &[8, 3, 7]).await["image"]["placeholder"], "AQI=");
    assert_eq!(created(&app, &[1, 2, 3]).await["image"]["id"], "1");

    // Back down to none.
    let playlist = created(&app, &[2, 3]).await;
    let (playlist, id) =
        edited(&app, &playlist, json!([{ "op": "remove", "positions": [0, 1] }])).await;
    assert!(id.is_null(), "{playlist}");
}

/// Fetches an image, which must be one JPEG, and returns its dimensions.
async fn dimensions(app: &Router, uri: &str) -> (u32, u32) {
    let (status, content_type, body) = send(app.clone(), "GET", uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}");
    assert_eq!(content_type.as_deref(), Some("image/jpeg"), "{uri}");
    let info = describe_image(&body).unwrap();
    (info.width, info.height)
}

#[tokio::test]
async fn a_mosaic_is_served_at_every_size_and_never_enlarged_past_its_covers() {
    let (temp, _db, app) = covers().await;
    let playlist = created(&app, &[1, 2, 3, 4]).await;
    let id = playlist["image"]["id"].as_str().unwrap();
    // Covers of 40 pixels fill a 2×2 grid of at most 80, which the first size to hold it, 128,
    // caps.
    for (asked, made) in [(16, 64), (64, 64), (100, 128), (128, 128), (512, 128), (4096, 128)] {
        let uri = format!("/api/v1/libraries/1/images/{id}?size={asked}");
        assert_eq!(dimensions(&app, &uri).await, (made, made), "{uri}");
    }
    let mut cached: Vec<_> = std::fs::read_dir(temp.path().join("cache/1/m"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    cached.sort();
    assert_eq!(cached, ["1-2-3-4-128.jpg", "1-2-3-4-64.jpg"]);

    // One cover is the image itself.
    let single = created(&app, &[5]).await;
    let uri = format!("/api/v1/libraries/1/images/{}", single["image"]["id"].as_str().unwrap());
    assert_eq!(dimensions(&app, &uri).await, (40, 30));
}

#[tokio::test]
async fn a_mosaic_names_only_its_own_librarys_images() {
    let (_temp, _db, app) = covers().await;
    // A playlist holds m1-2-3-4, so that is not why the others are refused.
    created(&app, &[1, 2, 3, 4]).await;
    for id in [
        // Image 20 is library 2's, and image 9 is nowhere.
        "m1-2-3-20",
        "m1-2-3-9",
        // Not four, not distinct, not canonical, or not a mosaic at all.
        "m1-2-3",
        "m1-2-3-4-5",
        "m1-1-2-3",
        "m01-2-3-4",
        "m+1-2-3-4",
        "m1-2-3-x",
        "m-1-2-3-4",
        "m1--2-3-4",
        "m",
        "M1-2-3-4",
        "1-2-3-4",
        "mm1-2-3-4",
    ] {
        let uri = format!("/api/v1/libraries/1/images/{id}");
        let (status, _, body) = send(app.clone(), "GET", &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{id}: {}", String::from_utf8_lossy(&body));
    }
    // Library 2 cannot reach library 1's.
    let (status, _, _) = send(app, "GET", "/api/v1/libraries/2/images/m1-2-3-4").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_cover_that_is_gone_makes_the_mosaic_not_found_and_that_is_not_remembered() {
    let (temp, _db, app) = covers().await;
    created(&app, &[1, 2, 3, 4]).await;
    let uri = "/api/v1/libraries/1/images/m1-2-3-4?size=64";
    std::fs::rename(temp.path().join("music/c3.png"), temp.path().join("c3.png")).unwrap();
    assert_eq!(send(app.clone(), "GET", uri).await.0, StatusCode::NOT_FOUND);
    std::fs::rename(temp.path().join("c3.png"), temp.path().join("music/c3.png")).unwrap();
    assert_eq!(dimensions(&app, uri).await, (64, 64));
}

#[tokio::test]
async fn the_sweep_keeps_mosaics_of_held_images_only() {
    let (temp, db, app) = covers().await;
    created(&app, &[1, 2, 3, 4]).await;
    assert_eq!(dimensions(&app, "/api/v1/libraries/1/images/m1-2-3-4?size=64").await, (64, 64));
    let cache = temp.path().join("cache");
    let kept = "1/m/1-2-3-4-64.jpg";
    assert!(cache.join(kept).exists());
    let unheld =
        ["1/m/1-2-3-9-64.jpg", "1/m/1-2-3-20-64.jpg", "1/m/1-2-3-4-64.failed", "1/m/stray"];
    for file in unheld {
        std::fs::write(cache.join(file), "").unwrap();
    }
    Images::new(cache.clone(), jewelcase_ffmpeg::Config::default()).sweep(&db).await.unwrap();
    assert!(cache.join(kept).exists());
    for file in unheld {
        assert!(!cache.join(file).exists(), "{file} kept");
    }
    // An image the library no longer holds takes its mosaics with it.
    db.write(|tx| {
        tx.execute("UPDATE albums SET image_id = NULL WHERE image_id = 3", [])?;
        tx.execute("DELETE FROM images WHERE id = 3", [])
    })
    .await
    .unwrap();
    Images::new(cache.clone(), jewelcase_ffmpeg::Config::default()).sweep(&db).await.unwrap();
    assert!(!cache.join(kept).exists());
}

#[tokio::test]
async fn a_mosaic_is_served_only_to_the_owner_of_a_playlist_that_holds_it() {
    let (temp, _db, app) = covers().await;
    add_others(&temp);
    let uri = "/api/v1/libraries/1/images/m1-2-3-4?size=64";
    let as_user = |token: &'static str| {
        let app = app.clone();
        async move {
            let request = axum::http::Request::get(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap();
            super::super::super::testing::respond(app, request).await.0
        }
    };
    // No playlist holds it yet, for anyone, and nothing is made.
    assert_eq!(as_user(TOKEN).await, StatusCode::NOT_FOUND);
    assert!(!temp.path().join("cache/1/m").exists());

    let playlist = created(&app, &[1, 2, 3, 4]).await;
    assert_eq!(as_user(TOKEN).await, StatusCode::OK);
    // Another account in the same library finds it as absent as one that never was.
    assert_eq!(as_user("user-token").await, StatusCode::NOT_FOUND);
    assert_eq!(as_user("admin-token").await, StatusCode::NOT_FOUND);

    // Once the playlist's artwork moves on, the old mosaic is no longer anyone's.
    edited(&app, &playlist, json!([{ "op": "add", "items": tracks_of(&[5]), "at": 0 }])).await;
    assert_eq!(as_user(TOKEN).await, StatusCode::NOT_FOUND);
}
