//! Waveforms: `getWaveform` (`api/openapi.yaml`).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use jewelcase_core::shape_waveform;
use rusqlite::OptionalExtension;
use serde::Serialize;

use super::extract::Path;
use super::scope::Scope;
use super::{Id, Problem};
use crate::db::{Database, waveform};

/// The spec's `Waveform`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Waveform {
    point_count: usize,
    data: String,
}

/// `getWaveform`: the track's shaped RMS levels, no content while it has no waveform this server
/// reads, and not found when the library has no such track.
pub async fn get(
    State(db): State<Arc<Database>>,
    Scope { library, .. }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
) -> Result<Response, Problem> {
    // No track is `None`; a track not yet analyzed is `Some(None)`.
    let blob: Option<Option<Vec<u8>>> = db
        .read(move |conn| {
            conn.prepare_cached(
                "SELECT w.data FROM tracks t LEFT JOIN track_waveforms w ON w.track_id = t.id \
                 WHERE t.library_id = ?1 AND t.id = ?2",
            )?
            .query_row([library, track], |row| row.get(0))
            .optional()
        })
        .await?;
    let blob = blob.ok_or_else(Problem::not_found)?;
    Ok(match blob.as_deref().and_then(waveform::decode) {
        Some(rms) => {
            let points = shape_waveform(rms);
            Json(Waveform { point_count: points.len(), data: BASE64.encode(&points) })
                .into_response()
        }
        None => StatusCode::NO_CONTENT.into_response(),
    })
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use super::super::testing::{app_with_tracks, send};
    use super::*;

    /// The points are the RMS levels shaped by `jewelcase_core`: a quiet passage 40 dB down and
    /// a loud one 6 dB down both stay visible.
    #[tokio::test]
    async fn serves_the_shaped_rms_levels() {
        let (_temp, db, app) = app_with_tracks().await;
        let blob = waveform::encode(&[[175; 4], [243; 4]].concat());
        db.write(move |tx| {
            tx.execute(
                "INSERT INTO track_waveforms (library_id, track_id, data) VALUES (1, 1, ?1)",
                [blob],
            )
        })
        .await
        .unwrap();

        let (status, _, body) = send(app, "GET", "/api/v1/libraries/1/tracks/1/waveform").await;
        assert_eq!(status, StatusCode::OK);
        let body: Value = serde_json::from_slice(&body).unwrap();
        let expected = BASE64.encode([24, 24, 24, 24, 255, 255, 255, 255]);
        assert_eq!(body, json!({ "pointCount": 8, "data": expected }));
    }

    #[tokio::test]
    async fn not_yet_analyzed_is_no_content() {
        let (_temp, db, app) = app_with_tracks().await;
        // Track 2 has a blob in a format version this server does not read.
        db.write(|tx| {
            tx.execute(
                "INSERT INTO track_waveforms (library_id, track_id, data) VALUES (1, 2, x'09')",
                [],
            )
        })
        .await
        .unwrap();
        for uri in
            ["/api/v1/libraries/1/tracks/1/waveform", "/api/v1/libraries/1/tracks/2/waveform"]
        {
            let (status, _, body) = send(app.clone(), "GET", uri).await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{uri}");
            assert!(body.is_empty(), "{uri}");
        }
    }

    #[tokio::test]
    async fn only_the_callers_library_is_reachable() {
        let (_temp, db, app) = app_with_tracks().await;
        let blob = waveform::encode(&[1]);
        db.write(move |tx| {
            tx.execute(
                "INSERT INTO track_waveforms (library_id, track_id, data) VALUES (2, 3, ?1)",
                [blob],
            )
        })
        .await
        .unwrap();
        for uri in [
            "/api/v1/libraries/1/tracks/3/waveform",
            "/api/v1/libraries/2/tracks/1/waveform",
            "/api/v1/libraries/1/tracks/404/waveform",
        ] {
            let (status, _, _) = send(app.clone(), "GET", uri).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        }
    }
}
