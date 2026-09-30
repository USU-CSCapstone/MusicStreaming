//! Lyrics: `getLyrics` (`api/openapi.yaml`).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_core::lrc::SyncedLine;
use rusqlite::OptionalExtension;
use rusqlite::types::Type;
use serde::Serialize;

use super::extract::Path;
use super::{Id, Problem};
use crate::db::Database;

/// The spec's `Lyrics`.
#[derive(Serialize)]
pub struct Lyrics {
    kind: &'static str,
    lines: Option<Vec<SyncedLine>>,
    plain: Option<String>,
}

pub async fn get(
    State(db): State<Arc<Database>>,
    Path((Id(library), Id(track))): Path<(Id, Id)>,
) -> Result<Json<Lyrics>, Problem> {
    db.read(move |conn| {
        conn.prepare_cached(
            "SELECT l.plain, l.synced FROM tracks t LEFT JOIN track_lyrics l ON l.track_id = t.id \
             WHERE t.library_id = ?1 AND t.id = ?2",
        )?
        .query_row([library, track], |row| {
            let plain: Option<String> = row.get(0)?;
            let synced: Option<String> = row.get(1)?;
            Ok(match synced {
                // `plain` holds the LRC text the lines came from, not a plain version.
                Some(synced) => Lyrics {
                    kind: "synced",
                    lines: Some(serde_json::from_str(&synced).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(1, Type::Text, Box::new(error))
                    })?),
                    plain: None,
                },
                None => Lyrics {
                    kind: if plain.is_some() { "plain" } else { "none" },
                    lines: None,
                    plain,
                },
            })
        })
        .optional()
    })
    .await?
    .map(Json)
    .ok_or_else(Problem::not_found)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use super::super::testing::{app_with_tracks, send};

    #[tokio::test]
    async fn serves_each_kind() {
        let (_temp, db, app) = app_with_tracks().await;
        db.write(|tx| {
            tx.execute_batch(
                r#"INSERT INTO track_lyrics (library_id, track_id, plain, synced, embedded) VALUES
                       (1, 1, '[00:01.00]Hello', '[{"startMs":1000,"text":"Hello"}]', 1),
                       (1, 2, 'Hello', NULL, 0);"#,
            )
        })
        .await
        .unwrap();
        for (track, expected) in [
            (
                1,
                json!({ "kind": "synced", "lines": [{ "startMs": 1000, "text": "Hello" }],
                        "plain": null }),
            ),
            (2, json!({ "kind": "plain", "lines": null, "plain": "Hello" })),
            // Library 2's track 3 has none.
            (3, json!({ "kind": "none", "lines": null, "plain": null })),
        ] {
            let library = if track == 3 { 2 } else { 1 };
            let uri = format!("/api/v1/libraries/{library}/tracks/{track}/lyrics");
            let (status, _, body) = send(app.clone(), "GET", &uri).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body, expected, "{uri}");
        }
    }

    #[tokio::test]
    async fn only_the_callers_library_is_reachable() {
        let (_temp, _db, app) = app_with_tracks().await;
        for uri in [
            "/api/v1/libraries/1/tracks/3/lyrics",
            "/api/v1/libraries/2/tracks/1/lyrics",
            "/api/v1/libraries/1/tracks/404/lyrics",
        ] {
            let (status, _, _) = send(app.clone(), "GET", uri).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        }
    }
}
