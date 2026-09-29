//! Waveforms: `getWaveform` (`api/openapi.yaml`).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use jewelcase_scanner::Waveform as Measured;
use rusqlite::OptionalExtension;
use serde::Serialize;

use super::{Code, Id, Problem};
use crate::db::Database;

/// The spec's `Waveform`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Waveform {
    point_count: usize,
    data: String,
}

pub async fn get(
    State(db): State<Arc<Database>>,
    Path((library_id, track_id)): Path<(String, String)>,
) -> Result<Response, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    let Id(track) = Id::parse(&track_id)?;
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
    let blob = blob.ok_or_else(|| Problem::new(Code::NotFound))?;
    // A blob from an analyzer version this server does not read counts as not analyzed: the
    // analyzer reprocesses old versions in the background (`design/scanning.md` §12).
    Ok(match blob.as_deref().and_then(Measured::from_blob) {
        Some(measured) => Json(shape(&measured.peaks)).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    })
}

/// Shapes the measured peaks for legibility (`requirements/playback.md` §3). Scaling to the
/// track's own loudest peak keeps a quiet recording from drawing as a flat line without
/// inventing structure: every point keeps its place and relative height.
fn shape(peaks: &[u8]) -> Waveform {
    let max = u32::from(peaks.iter().copied().max().unwrap_or(0));
    let points: Vec<u8> = if max == 0 {
        peaks.to_vec()
    } else {
        // Rounded to the nearest step, in integers.
        peaks
            .iter()
            .map(|&peak| ((u32::from(peak) * 255 + max / 2) / max) as u8)
            .collect()
    };
    Waveform {
        point_count: points.len(),
        data: BASE64.encode(&points),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use super::super::tests::{app_with_tracks, send};
    use super::*;

    #[tokio::test]
    async fn serves_the_shaped_peaks() {
        let (_temp, db, app) = app_with_tracks().await;
        let measured = Measured {
            peaks: vec![0, 32, 64, 128],
            rms: vec![0, 16, 32, 64],
        };
        let blob = measured.to_blob();
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
        // 32/128 of 255 is 63.75, and 64/128 is 127.5, which rounds up.
        let expected = BASE64.encode([0, 64, 128, 255]);
        assert_eq!(body, json!({ "pointCount": 4, "data": expected }));
    }

    #[tokio::test]
    async fn silence_stays_flat() {
        assert_eq!(shape(&[0, 0, 0]).data, BASE64.encode([0, 0, 0]));
        assert_eq!(shape(&[]).point_count, 0);
    }

    #[tokio::test]
    async fn not_yet_analyzed_is_no_content() {
        let (_temp, db, app) = app_with_tracks().await;
        // Track 2 has a blob from an analyzer version this server does not read.
        db.write(|tx| {
            tx.execute(
                "INSERT INTO track_waveforms (library_id, track_id, data) VALUES (1, 2, x'09')",
                [],
            )
        })
        .await
        .unwrap();
        for uri in [
            "/api/v1/libraries/1/tracks/1/waveform",
            "/api/v1/libraries/1/tracks/2/waveform",
        ] {
            let (status, _, body) = send(app.clone(), "GET", uri).await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{uri}");
            assert!(body.is_empty(), "{uri}");
        }
    }

    #[tokio::test]
    async fn only_the_callers_library_is_reachable() {
        let (_temp, db, app) = app_with_tracks().await;
        let blob = Measured {
            peaks: vec![1],
            rms: vec![1],
        }
        .to_blob();
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
