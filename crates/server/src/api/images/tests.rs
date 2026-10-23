use std::path::Path;

use axum::Router;
use axum::http::StatusCode;
use jewelcase_ffmpeg::Config;
use jewelcase_scanner::describe_image;
use serde_json::Value;

use super::super::testing::{TOKEN, app, ffmpeg, owner_request, send};
use super::cache::Images;
use crate::db::testing::{Image, library};
use crate::db::{Database, libraries};

/// Library 1 holds image 1, a 300×200 `cover.png`; image 2, a 64×64 picture embedded in
/// `song.mp3`; image 3, a `broken.gif` that is not an image; image 5, recorded with no
/// content hash; image 6, a `linked.png` that links to library 2's cover; and images 7 and 8, a
/// `huge.png` and an `animated.gif` that only their tests write. Library 2 holds image 4, a copy
/// of library 1's cover, with the same content hash.
async fn app_with_images() -> (tempfile::TempDir, Router) {
    let (temp, db, app) = app("");
    let (music, other) = (temp.path().join("music"), temp.path().join("other"));
    std::fs::create_dir(&music).unwrap();
    std::fs::create_dir(&other).unwrap();
    let cover = music.join("cover.png");
    let cover = cover.to_str().unwrap();
    ffmpeg(&["-f", "lavfi", "-i", "color=red:s=300x200", "-frames:v", "1", cover]);
    let small = temp.path().join("small.png");
    let small = small.to_str().unwrap();
    ffmpeg(&["-f", "lavfi", "-i", "color=blue:s=64x64", "-frames:v", "1", small]);
    let song = music.join("song.mp3");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=duration=1",
        "-i",
        small,
        "-map",
        "0",
        "-map",
        "1",
        "-c:v",
        "png",
        "-disposition:v",
        "attached_pic",
        song.to_str().unwrap(),
    ]);
    std::fs::write(music.join("broken.gif"), b"not an image").unwrap();
    std::fs::copy(cover, other.join("cover.png")).unwrap();
    std::os::unix::fs::symlink(other.join("cover.png"), music.join("linked.png")).unwrap();
    db.write(move |tx| {
        library(tx, 1, "Music", music)?;
        library(tx, 2, "Other", other)?;
        // The cache names each rendering after its library and its image's hash, such as
        // `1/01/01-128.jpg`.
        let cover =
            |id, library| Image { width: 300, height: 200, ..Image::new(id, library, "cover.png") };
        Image { hash: vec![1], ..cover(1, 1) }.insert(tx)?;
        Image { width: 64, height: 64, ..Image::new(2, 1, "song.mp3") }.insert(tx)?;
        Image { hash: vec![3], width: 512, height: 512, ..Image::new(3, 1, "broken.gif") }
            .insert(tx)?;
        Image { hash: vec![1], ..cover(4, 2) }.insert(tx)?;
        Image { hash: Vec::new(), ..cover(5, 1) }.insert(tx)?;
        Image { hash: vec![6], path: "linked.png", ..cover(6, 1) }.insert(tx)?;
        let huge = Image::new(7, 1, "huge.png");
        Image { hash: vec![7], width: 8192, height: 8193, ..huge }.insert(tx)?;
        let animated = Image::new(8, 1, "animated.gif");
        Image { hash: vec![8], width: 32, height: 32, ..animated }.insert(tx)
    })
    .await
    .unwrap();
    (temp, app)
}

/// Fetches an image, which must be one JPEG, and returns its dimensions.
async fn dimensions(app: &Router, uri: &str) -> (u32, u32) {
    let (status, content_type, body) = send(app.clone(), "GET", uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}");
    assert_eq!(content_type.as_deref(), Some("image/jpeg"), "{uri}");
    // JPEG's start-of-image marker, then the next marker.
    assert!(body.starts_with(&[0xFF, 0xD8, 0xFF]), "{uri}");
    let starts = body.windows(3).filter(|bytes| bytes == &[0xFF, 0xD8, 0xFF]).count();
    assert_eq!(starts, 1, "{uri} holds {starts} JPEGs");
    let info = describe_image(&body).unwrap();
    (info.width, info.height)
}

#[tokio::test]
async fn resizes_to_the_size_above_the_request() {
    let (temp, app) = app_with_images().await;
    // 100 rounds up to 128; the aspect ratio holds.
    assert_eq!(dimensions(&app, "/api/v1/libraries/1/images/1?size=100").await, (128, 85));
    // Never enlarged past the original, so every size above the one that holds it whole is
    // that one, made once.
    for size in [512, 1024, 2048, 4096] {
        let uri = format!("/api/v1/libraries/1/images/1?size={size}");
        assert_eq!(dimensions(&app, &uri).await, (300, 200), "{uri}");
    }
    // The embedded picture, at the default size.
    assert_eq!(dimensions(&app, "/api/v1/libraries/1/images/2").await, (64, 64));

    // Served from the cache once made, with nothing left behind.
    std::fs::remove_file(temp.path().join("music/cover.png")).unwrap();
    assert_eq!(dimensions(&app, "/api/v1/libraries/1/images/1?size=128").await, (128, 85));
    let mut cached: Vec<_> = std::fs::read_dir(temp.path().join("cache/1/01"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    cached.sort();
    assert_eq!(cached, ["01-128.jpg", "01-512.jpg"]);
}

/// Two libraries holding the same artwork each make and keep their own copy, so neither can
/// tell by how fast it is answered whether the other has viewed it.
#[tokio::test]
async fn each_library_keeps_its_own_copies() {
    let (temp, app) = app_with_images().await;
    assert_eq!(dimensions(&app, "/api/v1/libraries/1/images/1?size=64").await, (64, 43));
    assert!(!temp.path().join("cache/2").exists());
    assert_eq!(dimensions(&app, "/api/v1/libraries/2/images/4?size=64").await, (64, 43));
    assert!(temp.path().join("cache/2/01/01-64.jpg").exists());
}

#[tokio::test]
async fn is_cacheable_by_the_client_only() {
    use axum::body::Body;
    use axum::http::{Request, header};

    let (_temp, app) = app_with_images().await;
    let uri = "/api/v1/libraries/1/images/1?size=64";
    // By cookie, while the device is not seen lately: the cookie is sent again, and the image
    // keeps its own `Cache-Control`.
    let cookie = format!("jewelcase_session={TOKEN}");
    let request = Request::get(uri).header(header::COOKIE, cookie);
    let response = tower::ServiceExt::oneshot(app.clone(), request.body(Body::empty()).unwrap());
    let response = response.await.unwrap();
    assert!(response.headers().contains_key(header::SET_COOKIE));
    let cache_control: Vec<_> = response.headers().get_all(header::CACHE_CONTROL).iter().collect();
    assert_eq!(cache_control, ["private, max-age=31536000, immutable"]);

    let request = owner_request("GET", uri).body(Body::empty()).unwrap();
    let response = tower::ServiceExt::oneshot(app, request).await.unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "private, max-age=31536000, immutable");
}

#[tokio::test]
async fn problems() {
    let (temp, app) = app_with_images().await;
    for (uri, status, code) in [
        // Image 4 in library 1, image 1 in library 2, and one that does not exist.
        ("/api/v1/libraries/1/images/4", StatusCode::NOT_FOUND, "not_found"),
        ("/api/v1/libraries/2/images/1", StatusCode::NOT_FOUND, "not_found"),
        ("/api/v1/libraries/1/images/9", StatusCode::NOT_FOUND, "not_found"),
        (
            "/api/v1/libraries/1/images/1?size=15",
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
        ),
        (
            "/api/v1/libraries/1/images/1?size=4097",
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
        ),
        (
            "/api/v1/libraries/1/images/1?width=64",
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
        ),
        // ffmpeg's refusal names the file; it stays in the log.
        ("/api/v1/libraries/1/images/3", StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        ("/api/v1/libraries/1/images/5", StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    ] {
        let (actual, _, body) = send(app.clone(), "GET", uri).await;
        assert_eq!(actual, status, "{uri}");
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains(temp.path().to_str().unwrap()), "{uri}: {text}");
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], code, "{uri}");
    }
}

#[tokio::test]
async fn an_unreadable_image_is_an_internal_problem_that_names_no_path() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, app) = app_with_images().await;
    let cover = temp.path().join("music/cover.png");
    std::fs::set_permissions(&cover, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&cover).is_ok() {
        eprintln!("skipping: permissions do not apply to this user");
        return;
    }
    let uri = "/api/v1/libraries/1/images/1?size=64";
    let (status, _, body) = send(app.clone(), "GET", uri).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let body = String::from_utf8(body).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["code"], "internal");
    assert!(!body.contains(temp.path().to_str().unwrap()), "{body}");
    // Not remembered as broken: readable again, it renders.
    std::fs::set_permissions(&cover, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(dimensions(&app, uri).await, (64, 43));
}

#[tokio::test]
async fn a_pipe_is_refused_without_waiting_for_a_writer() {
    let (temp, app) = app_with_images().await;
    let cover = temp.path().join("music/cover.png");
    std::fs::remove_file(&cover).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(&cover).status().unwrap().success());
    let request = send(app, "GET", "/api/v1/libraries/1/images/1?size=64");
    let answered = tokio::time::timeout(std::time::Duration::from_secs(5), request).await;
    if answered.is_err() {
        // Opening the pipe to write lets go of a reader stuck on it, so the test can end. It is
        // opened on a thread of its own, since no reader may be left to let it go.
        let pipe = cover.clone();
        std::thread::spawn(move || std::fs::OpenOptions::new().write(true).open(pipe).unwrap());
    }
    let (status, _, _) = answered.expect("answered without waiting on the pipe");
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!temp.path().join("cache/1/01/01-64.failed").exists(), "not remembered as broken");
    std::fs::remove_file(&cover).unwrap();
}

#[tokio::test]
async fn a_broken_image_is_not_tried_again() {
    let (temp, app) = app_with_images().await;
    let uri = "/api/v1/libraries/1/images/3";
    let (status, _, _) = send(app.clone(), "GET", uri).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(temp.path().join("cache/1/03/03-512.failed").exists());
    // Readable now, but its content hash says it is the same broken image, so ffmpeg is not run.
    let music = temp.path().join("music");
    std::fs::copy(music.join("cover.png"), music.join("broken.gif")).unwrap();
    let (status, _, _) = send(app, "GET", uri).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn an_image_past_the_pixel_cap_is_refused_and_not_tried_again() {
    let (temp, app) = app_with_images().await;
    // One row more than 8192², the cap; one bit a pixel keeps it quick to write.
    let huge = temp.path().join("music/huge.png");
    let huge = huge.to_str().unwrap();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=black:s=8192x8193",
        "-frames:v",
        "1",
        "-pix_fmt",
        "monob",
        huge,
    ]);
    let (status, _, _) = send(app, "GET", "/api/v1/libraries/1/images/7").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(temp.path().join("cache/1/07/07-512.failed").exists());
}

#[tokio::test]
async fn an_animated_image_is_its_first_frame() {
    let (temp, app) = app_with_images().await;
    // Two frames, a tenth of a second each.
    let animated = temp.path().join("music/animated.gif");
    ffmpeg(&["-f", "lavfi", "-i", "color=red:s=32x32:d=0.2:r=10", animated.to_str().unwrap()]);
    assert_eq!(dimensions(&app, "/api/v1/libraries/1/images/8?size=64").await, (32, 32));
}

/// An image as large as one of the sizes is made at that size alone, whatever larger size is
/// asked for.
#[tokio::test]
async fn an_image_the_size_of_a_size_is_made_at_that_size() {
    let (temp, app) = app_with_images().await;
    let square = temp.path().join("music/square.png");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=red:s=512x512",
        "-frames:v",
        "1",
        square.to_str().unwrap(),
    ]);
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    let image = Image::new(9, 1, "square.png");
    Image { hash: vec![9], width: 512, height: 512, ..image }.insert(&conn).unwrap();
    let uri = "/api/v1/libraries/1/images/9?size=1024";
    assert_eq!(dimensions(&app, uri).await, (512, 512));
    let cached: Vec<_> = std::fs::read_dir(temp.path().join("cache/1/09"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(cached, ["09-512.jpg"]);
}

/// An image whose file is missing, as on a share not mounted, is not found, as a track's is,
/// and is tried again once the file is back.
#[tokio::test]
async fn a_missing_file_is_not_found_and_tried_again() {
    let (temp, app) = app_with_images().await;
    let (cover, spare) = (temp.path().join("music/cover.png"), temp.path().join("spare.png"));
    std::fs::rename(&cover, &spare).unwrap();
    let uri = "/api/v1/libraries/1/images/1?size=64";
    let (status, _, body) = send(app.clone(), "GET", uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["code"], "not_found");
    std::fs::rename(&spare, &cover).unwrap();
    assert_eq!(dimensions(&app, uri).await, (64, 43));
}

/// A file that goes away as ffmpeg reads it, as on a share that drops, is not remembered as a
/// broken image: it is not found, and tried again once it is back.
#[tokio::test]
async fn a_file_lost_while_it_is_read_is_not_remembered_as_broken() {
    let (temp, _app) = app_with_images().await;
    let stand_in = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vanishing-ffmpeg.sh");
    let config = Config { ffmpeg: stand_in.clone(), ffprobe: stand_in };
    let images = Images::new(temp.path().join("cache"), config);
    let db = Database::open(&temp.path().join("jewelcase.db")).unwrap();
    let source = db
        .read(|conn| {
            let root =
                conn.query_row("SELECT id FROM library_roots WHERE library_id = 1", [], |r| {
                    r.get(0)
                })?;
            libraries::file(conn, 1, root, "cover.png")
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(images.resized(1, &[1], 64, source).await.unwrap(), None);
    assert!(!temp.path().join("music/cover.png").exists(), "the stand-in read the cover");
    assert!(!temp.path().join("cache/1/01/01-64.failed").exists(), "remembered as broken");
}

/// A link is not followed, so what it leads to is not found, and nothing is rendered or
/// remembered.
#[tokio::test]
async fn a_link_out_of_the_library_is_not_found() {
    let (temp, app) = app_with_images().await;
    let (status, _, _) = send(app, "GET", "/api/v1/libraries/1/images/6").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!temp.path().join("cache/1/06").exists(), "nothing rendered or remembered");
}

/// The sweep keeps each library's copies of the images it holds, and deletes the rest: copies
/// of an image its library does not hold, even one another library holds, failure markers,
/// the folder of a library that is gone, and the files of renders cut short.
#[tokio::test]
async fn the_sweep_deletes_what_no_image_needs() {
    let (temp, app) = app_with_images().await;
    let cache = temp.path().join("cache");
    assert_eq!(dimensions(&app, "/api/v1/libraries/1/images/1?size=64").await, (64, 43));
    let (status, _, _) = send(app, "GET", "/api/v1/libraries/1/images/3").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(cache.join("1/03/03-512.failed").exists());
    // An image only library 2 holds.
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    Image { hash: vec![0x22], ..Image::new(9, 2, "cover.png") }.insert(&conn).unwrap();
    let kept = ["1/01/01-64.jpg", "2/22/22-64.jpg"];
    let unheld = [
        "1/03/03-512.failed",
        "1/22/22-64.jpg",
        "1/ff/ff-64.jpg",
        "1/01/.tmpAbC123",
        "1/stray",
        "9/01/01-64.jpg",
    ];
    for file in [kept[1]].into_iter().chain(unheld) {
        let path = cache.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    let db = Database::open(&temp.path().join("jewelcase.db")).unwrap();
    Images::new(cache.clone(), Config::default()).sweep(&db).await.unwrap();
    for file in kept {
        assert!(cache.join(file).exists(), "{file} deleted");
    }
    for file in unheld {
        assert!(!cache.join(file).exists(), "{file} kept");
    }
    assert!(!cache.join("9").exists(), "a library that is gone keeps its folder");
}

/// An image under a root removed from its library is not served, as if it did not exist.
#[tokio::test]
async fn an_image_under_a_removed_root_is_not_found() {
    let (temp, app) = app_with_images().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    conn.execute("UPDATE library_roots SET removed_at = 1 WHERE library_id = 1", []).unwrap();
    let (status, _, _) = send(app, "GET", "/api/v1/libraries/1/images/1").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
