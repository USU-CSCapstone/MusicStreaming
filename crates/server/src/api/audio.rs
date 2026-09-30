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
                        loudness: loudness(row)?,
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
mod tests;
