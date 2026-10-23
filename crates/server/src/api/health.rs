//! `getHealth`: whether the server can serve, for container orchestration
//! (`requirements/deployment.md` §7).

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::{Value, json};
use tokio::time::timeout;
use tracing::warn;

use crate::db::Database;

/// How long a database reader may take to answer before the server counts as unable to serve:
/// far longer than any read takes, and shorter than an orchestrator waits for the answer.
const READ_WITHIN: Duration = Duration::from_secs(2);

/// `200 {status: "ok"}` once a database reader has opened a read transaction on the database, or
/// `503 {status: "unavailable"}` if it fails or takes longer than [`READ_WITHIN`]. It reads the
/// schema version from the database header, which every database has, even before setup.
pub async fn health(State(db): State<Arc<Database>>) -> (StatusCode, Json<Value>) {
    let read = db.read(|conn| conn.query_row("PRAGMA user_version", [], |_| Ok(())));
    match timeout(READ_WITHIN, read).await {
        Ok(Ok(())) => return (StatusCode::OK, Json(json!({ "status": "ok" }))),
        Ok(Err(error)) => warn!(%error, "the health check cannot read the database"),
        Err(_) => warn!("the health check's database read took over {READ_WITHIN:?}"),
    }
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "unavailable" })))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use futures_util::FutureExt;
    use serde_json::{Value, json};
    use tokio::sync::RwLock;

    use super::super::testing::{app, send};
    use super::*;

    /// The status and JSON body of `GET /health`.
    async fn check(app: axum::Router) -> (StatusCode, Value) {
        let (status, _, body) = send(app, "GET", "/api/v1/health").await;
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn a_database_that_answers_is_healthy() {
        let (_temp, _db, app) = app("");
        assert_eq!(check(app).await, (StatusCode::OK, json!({ "status": "ok" })));
    }

    /// Paused, so the wait for the read runs out at once.
    #[tokio::test(start_paused = true)]
    async fn a_database_that_does_not_answer_is_unavailable() {
        let (_temp, db, app) = app("");
        // Every reader is kept busy until `held` is dropped: more jobs are queued ahead of the
        // check than there are readers, each waiting to read `gate`.
        let gate = Arc::new(RwLock::new(()));
        let held = gate.write().await;
        for _ in 0..256 {
            let gate = Arc::clone(&gate);
            let busy = db.read(move |_| {
                drop(gate.blocking_read());
                Ok(())
            });
            // Polled once, which queues the job; nothing waits for its answer.
            assert!(busy.now_or_never().is_none());
        }
        assert_eq!(
            check(app).await,
            (StatusCode::SERVICE_UNAVAILABLE, json!({ "status": "unavailable" }))
        );
        drop(held);
    }
}
