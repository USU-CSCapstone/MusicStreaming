//! Audio delivery: `getPlaybackInfo` and `getAudio` (`api/openapi.yaml`).
//!
//! Only direct delivery exists so far: the original file, whole or by byte range. A request the
//! original cannot answer, a lower quality or a codec the client cannot play, needs a transcode,
//! and answers `422` until transcoding is built.

use std::io;
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use jewelcase_core::Format;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tower_http::services::ServeFile;

use super::extract::{Path, Query};
use super::scope::Scope;
use super::tracks::{LOUDNESS, Loudness, loudness};
use super::{Code, Id, Problem};
use crate::db::{Database, libraries};

/// The one variant there is: the file as it is on disk.
const ORIGINAL: &str = "original";

/// What the client wants the audio for, which decides its [`Transport`].
#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Purpose {
    #[default]
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

/// The spec's `delivery`: only `direct`, the original file, until transcoding is built.
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Delivery {
    Direct,
}

/// The spec's `transport`: only `progressive`, one file read start to end, until segmented
/// transport is built.
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Transport {
    Progressive,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlaybackQuery {
    #[serde(default)]
    purpose: Purpose,
    quality: Quality,
    /// Comma-separated, most preferred first.
    codecs: Option<String>,
}

/// The spec's `PlaybackInfo`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackInfo {
    variant: &'static str,
    delivery: Delivery,
    codec: String,
    container: String,
    bitrate_kbps: Option<i64>,
    sample_rate_hz: i64,
    bit_depth: Option<i64>,
    channels: i64,
    size_bytes: Option<i64>,
    transport: Transport,
    loudness: Option<Loudness>,
    /// Always null: the scanner does not measure encoder delay and padding yet. `()` serializes
    /// as JSON `null`.
    gapless: (),
}

/// The spec's `getPlaybackInfo`: how the track's original plays, for the quality, codecs and
/// purpose asked. What the original cannot answer needs a transcode, and answers `422`.
pub async fn playback(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
    Query(query): Query<PlaybackQuery>,
) -> Result<Json<PlaybackInfo>, Problem> {
    let transcode = |detail| Err(Problem::invalid(detail));
    if query.quality != Quality::Original {
        return transcode("only original quality is available until transcoding is built");
    }
    // The original is a complete file of known size, so it answers both purposes the same.
    let transport = match query.purpose {
        Purpose::Stream | Purpose::Download => Transport::Progressive,
    };
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
                        delivery: Delivery::Direct,
                        codec: row.get(0)?,
                        container: row.get(1)?,
                        bitrate_kbps: row.get(2)?,
                        sample_rate_hz: row.get(3)?,
                        bit_depth: row.get(4)?,
                        channels: row.get(5)?,
                        size_bytes: Some(row.get(6)?),
                        transport,
                        loudness: loudness(row)?,
                        gapless: (),
                    })
                })
                .optional()
        })
        .await?
        .ok_or_else(Problem::not_found)?;

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

/// The spec's `getAudio`: the track's original file, whole or by byte range. A track that is
/// missing, or under a root no longer in its library, is `404`.
pub async fn audio(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
    Query(query): Query<AudioQuery>,
    request: Request,
) -> Result<Response, Problem> {
    if query.variant != ORIGINAL {
        return Err(Problem::not_found());
    }
    let (file, container) = db
        .try_read(move |conn| -> Result<_, Problem> {
            let (root, path, container) = conn
                .prepare_cached(
                    "SELECT root_id, path, container FROM tracks \
                     WHERE library_id = ?1 AND id = ?2 AND missing_since IS NULL",
                )?
                .query_row([library, track], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
                })
                .optional()?
                .ok_or_else(Problem::not_found)?;
            let file =
                libraries::file(conn, library, root, &path)?.ok_or_else(Problem::not_found)?;
            Ok((file, container))
        })
        .await?;

    // Checked on every request, since a file can become a symlink after the scan saw it.
    let path = tokio::task::spawn_blocking(move || file.resolve())
        .await
        .map_err(|_| Problem::new(Code::Internal))?
        .map_err(unreadable)?;
    // ServeFile answers ranges, `HEAD`, and the conditional headers, streaming from disk.
    let response = ServeFile::new(path).try_call(request).await.map_err(unreadable)?;
    let mut response = response.map(Body::new);
    // Private: a track is only for those who can reach its library.
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private"));
    // ServeFile guesses the type from the extension; the probe read the container from the
    // content. A container no format names keeps the guess.
    let format = Format::ALL.into_iter().find(|format| format.container_name() == container);
    if response.status().is_success()
        && let Some(format) = format
    {
        let media_type = HeaderValue::from_static(format.media_type());
        response.headers_mut().insert(header::CONTENT_TYPE, media_type);
    }
    Ok(response)
}

/// The answer for a track's file that cannot be read. One gone since the last scan saw it, or
/// reached through a symlink, is not found. Otherwise the cause stays in the log, since it
/// names a path on the host.
fn unreadable(error: io::Error) -> Problem {
    if error.kind() == io::ErrorKind::NotFound {
        return Problem::not_found();
    }
    tracing::error!(%error, "cannot read a track's file");
    Problem::new(Code::Internal)
}

#[cfg(test)]
mod tests;
