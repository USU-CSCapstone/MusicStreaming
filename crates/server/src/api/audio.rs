//! Audio delivery: `getPlaybackInfo` and `getAudio` (`api/openapi.yaml`).
//!
//! Only direct delivery exists so far: the original file, whole or by byte range. A request the
//! original cannot answer, a lower quality or a codec the client cannot play, needs a transcode,
//! and answers `422` until transcoding is built.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tower_http::services::ServeFile;

use super::query::Query;
use super::tracks::{LOUDNESS, Loudness, loudness};
use super::{Code, Id, Problem};
use crate::db::Database;

/// The one variant there is: the file as it is on disk.
const ORIGINAL: &str = "original";

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Purpose {
    Stream,
    Download,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Quality {
    Original,
    High,
    Medium,
    Low,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlaybackQuery {
    // The original is a complete file of known size, so it answers both purposes the same.
    #[expect(
        dead_code,
        reason = "accepted and checked, but direct delivery ignores it"
    )]
    purpose: Option<Purpose>,
    quality: Quality,
    /// Comma-separated, most preferred first.
    codecs: Option<String>,
}

/// The spec's `PlaybackInfo`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackInfo {
    variant: &'static str,
    delivery: &'static str,
    codec: String,
    container: String,
    bitrate_kbps: Option<i64>,
    sample_rate_hz: i64,
    bit_depth: Option<i64>,
    channels: i64,
    size_bytes: Option<i64>,
    transport: &'static str,
    loudness: Option<Loudness>,
    /// Always null: the scanner does not measure encoder delay and padding yet. `()` serializes
    /// as JSON `null`.
    gapless: (),
}

pub async fn playback(
    State(db): State<Arc<Database>>,
    Path((library_id, track_id)): Path<(String, String)>,
    Query(query): Query<PlaybackQuery>,
) -> Result<Json<PlaybackInfo>, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    let Id(track) = Id::parse(&track_id)?;
    let transcode = |detail| Err(Problem::new(Code::ValidationFailed).detail(detail));
    if query.quality != Quality::Original {
        return transcode("only original quality is available until transcoding is built");
    }
    let info = db
        .read(move |conn| {
            let sql = format!(
                "SELECT t.codec, t.container, t.bitrate_kbps, t.sample_rate_hz, t.bit_depth, \
                 t.channels, t.file_size, {LOUDNESS} \
                 FROM tracks t JOIN albums al ON al.id = t.album_id \
                 WHERE t.library_id = ?1 AND t.id = ?2 AND t.missing_since IS NULL"
            );
            conn.prepare_cached(&sql)?
                .query_row([library, track], |row| {
                    Ok(PlaybackInfo {
                        variant: ORIGINAL,
                        delivery: "direct",
                        codec: row.get(0)?,
                        container: row.get(1)?,
                        bitrate_kbps: row.get(2)?,
                        sample_rate_hz: row.get(3)?,
                        bit_depth: row.get(4)?,
                        channels: row.get(5)?,
                        size_bytes: Some(row.get(6)?),
                        transport: "progressive",
                        loudness: loudness(row, 7)?,
                        gapless: (),
                    })
                })
                .optional()
        })
        .await?
        .ok_or_else(|| Problem::new(Code::NotFound))?;

    // The client can play the original when it names both the codec and its container, since
    // a codec such as `pcm` plays in some containers and not others.
    if let Some(codecs) = &query.codecs {
        let codecs: Vec<&str> = codecs.split(',').collect();
        if !codecs.contains(&info.codec.as_str()) || !codecs.contains(&info.container.as_str()) {
            return transcode("the original needs a codec the client cannot play");
        }
    }
    Ok(Json(info))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioQuery {
    variant: String,
}

pub async fn audio(
    State(db): State<Arc<Database>>,
    Path((library_id, track_id)): Path<(String, String)>,
    Query(query): Query<AudioQuery>,
    request: Request,
) -> Result<Response, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    let Id(track) = Id::parse(&track_id)?;
    if query.variant != ORIGINAL {
        return Err(Problem::new(Code::NotFound));
    }
    let (root, path, container) = db
        .read(move |conn| {
            conn.prepare_cached(
                "SELECT r.path, t.path, t.container \
                 FROM tracks t JOIN library_roots r ON r.id = t.root_id \
                 WHERE t.library_id = ?1 AND t.id = ?2 AND t.missing_since IS NULL",
            )?
            .query_row([library, track], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .optional()
        })
        .await?
        .ok_or_else(|| Problem::new(Code::NotFound))?;

    // ServeFile answers ranges, `HEAD`, and the conditional headers, streaming from disk.
    let response = ServeFile::new(PathBuf::from(root).join(path))
        .try_call(request)
        .await
        .map_err(|error| match error.kind() {
            // The file has gone since the last scan saw it.
            ErrorKind::NotFound => Problem::new(Code::NotFound),
            _ => {
                // The cause stays in the log: it names a path on the host.
                tracing::error!(%error, "cannot read a track's file");
                Problem::new(Code::Internal)
            }
        })?;
    let mut response = response.map(Body::new);
    // ServeFile guesses the type from the extension; the probe read it from the content.
    if response.status().is_success() {
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(mime(&container)),
        );
    }
    Ok(response)
}

/// The media type for a container, named as `Format::container_name` names it.
fn mime(container: &str) -> &'static str {
    match container {
        "flac" => "audio/flac",
        "mp4" => "audio/mp4",
        "wav" => "audio/wav",
        "aiff" => "audio/aiff",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::super::tests::{app, send};
    use super::*;
    use crate::db::libraries;

    const AUDIO: &[u8] = b"0123456789";

    /// Library 1 holds track 1, AAC on disk as `a.m4a`, and track 2, missing since a scan.
    /// Library 2 holds track 3.
    async fn app_with_fixture() -> (tempfile::TempDir, Router) {
        let (temp, db, app) = app("");
        let (music, other) = (temp.path().join("music"), temp.path().join("other"));
        for dir in [&music, &other] {
            std::fs::create_dir(dir).unwrap();
            std::fs::write(dir.join("a.m4a"), AUDIO).unwrap();
        }
        db.write(move |tx| {
            libraries::create(tx, Some(1), "Music", &[&music], &[])?;
            libraries::create(tx, Some(2), "Other", &[&other], &[])?;
            tx.execute_batch(
                "INSERT INTO albums (id, library_id, title_key, artists_key, sort_key,
                                     artist_sort_key, loudness_lufs, peak_dbtp, added_at, updated_at)
                 VALUES (101, 1, 'a', '', x'61', x'', -9.5, -0.25, 0, 0),
                        (201, 2, 'a', '', x'61', x'', NULL, NULL, 0, 0);
                 WITH t (id, library_id, album_id, path, missing_since) AS (VALUES
                     (1, 1, 101, 'a.m4a', NULL), (2, 1, 101, 'b.m4a', 5), (3, 2, 201, 'a.m4a', NULL))
                 INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size, file_mtime,
                                     title, sort_key, artist_sort_key, album_sort_key, missing_since,
                                     codec, container, lossless, sample_rate_hz, bit_depth,
                                     channels, duration_us, loudness_lufs, peak_dbtp, added_at,
                                     updated_at)
                 SELECT t.id, t.library_id, t.album_id, r.id, t.path, 10, 0, 'A', x'61', x'',
                        x'61', t.missing_since, 'aac', 'mp4', 0, 44100, NULL, 2, 1000000, -10.5,
                        -0.5, 0, 0
                 FROM t JOIN library_roots r ON r.library_id = t.library_id;",
            )
        })
        .await
        .unwrap();
        (temp, app)
    }

    /// Sends a `GET` with a `Range` header.
    async fn get_range(app: Router, uri: &str, range: &str) -> Response {
        let request = Request::get(uri)
            .header(header::RANGE, range)
            .body(Body::empty())
            .unwrap();
        app.oneshot(request).await.unwrap()
    }

    fn json(body: &[u8]) -> Value {
        serde_json::from_slice(body).unwrap()
    }

    #[tokio::test]
    async fn playback_delivers_the_original() {
        let (_temp, app) = app_with_fixture().await;
        let (status, _, body) = send(
            app.clone(),
            "GET",
            "/api/v1/libraries/1/tracks/1/playback?quality=original&purpose=download&codecs=opus,aac,mp4",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json(&body),
            json!({
                "variant": "original",
                "delivery": "direct",
                "codec": "aac",
                "container": "mp4",
                "bitrateKbps": null,
                "sampleRateHz": 44100,
                "bitDepth": null,
                "channels": 2,
                "sizeBytes": 10,
                "transport": "progressive",
                "loudness": {
                    "trackLufs": -10.5,
                    "trackPeakDbtp": -0.5,
                    "albumLufs": -9.5,
                    "albumPeakDbtp": -0.25,
                },
                "gapless": null,
            })
        );
    }

    #[tokio::test]
    async fn playback_refuses_what_needs_a_transcode() {
        let (_temp, app) = app_with_fixture().await;
        for query in [
            "quality=high",
            "quality=original&codecs=mp3,opus",
            // The codec alone is not enough: the container must play too, and the other way round.
            "quality=original&codecs=aac,ogg",
            "quality=original&codecs=mp4",
            // Not a request that needs a transcode, but malformed.
            "purpose=stream",
            "quality=best",
            "quality=original&purpose=radio",
            "quality=original&extra=1",
        ] {
            let uri = format!("/api/v1/libraries/1/tracks/1/playback?{query}");
            let (status, _, body) = send(app.clone(), "GET", &uri).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
            assert_eq!(json(&body)["code"], "validation_failed", "{query}");
        }
    }

    #[tokio::test]
    async fn audio_serves_the_file_and_its_ranges() {
        let (_temp, app) = app_with_fixture().await;
        let uri = "/api/v1/libraries/1/tracks/1/audio?variant=original";
        let (status, content_type, body) = send(app.clone(), "GET", uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type.as_deref(), Some("audio/mp4"));
        assert_eq!(body, AUDIO);

        let response = get_range(app.clone(), uri, "bytes=2-5").await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        let headers = response.headers();
        assert_eq!(headers[header::CONTENT_TYPE], "audio/mp4");
        assert_eq!(headers[header::ACCEPT_RANGES], "bytes");
        assert_eq!(headers[header::CONTENT_RANGE], "bytes 2-5/10");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"2345");

        let response = get_range(app.clone(), uri, "bytes=20-").await;
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes */10");

        let (status, _, body) = send(app, "HEAD", uri).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn only_the_callers_library_is_reachable() {
        let (temp, app) = app_with_fixture().await;
        std::fs::remove_file(temp.path().join("other/a.m4a")).unwrap();
        // Track 3 in library 1, track 1 in library 2, track 2 missing, and an unknown variant.
        for uri in [
            "/api/v1/libraries/1/tracks/3/playback?quality=original",
            "/api/v1/libraries/2/tracks/1/playback?quality=original",
            "/api/v1/libraries/1/tracks/2/playback?quality=original",
            "/api/v1/libraries/1/tracks/3/audio?variant=original",
            "/api/v1/libraries/2/tracks/1/audio?variant=original",
            "/api/v1/libraries/1/tracks/2/audio?variant=original",
            "/api/v1/libraries/1/tracks/1/audio?variant=high",
            // The file is gone since the last scan.
            "/api/v1/libraries/2/tracks/3/audio?variant=original",
        ] {
            let (status, content_type, body) = send(app.clone(), "GET", uri).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(
                content_type.as_deref(),
                Some("application/problem+json"),
                "{uri}"
            );
            assert_eq!(json(&body)["code"], "not_found", "{uri}");
        }
    }
}
