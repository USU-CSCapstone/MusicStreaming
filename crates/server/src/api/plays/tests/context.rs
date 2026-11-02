//! A play records the context of the queue it was played from.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{PLAY, post, report};
use crate::api::testing::player::start_queue;
use crate::api::testing::{TOKEN, app_with_tracks, call};

/// The device holds the active queue, `context` and all, and a play of one of its items is
/// reported under that context, which is recorded as sent with the play's origin. The server
/// takes no context of its own: the queue may be another by the time a report arrives, as an
/// offline device catches up.
#[tokio::test]
async fn play_records_queue_context() {
    let (_temp, db, app) = app_with_tracks().await;
    start_queue(&app, TOKEN).await;
    let (status, queue) = call(&app, TOKEN, "GET", "/api/v1/me/queue", None).await;
    assert_eq!(status, StatusCode::OK);
    let context = queue["context"].clone();
    assert_eq!(context, json!({ "type": "album", "id": "101" }));

    // A track of the queue's list, started from its Manual Queue and then from its context.
    for (n, origin) in ["manual", "context"].into_iter().enumerate() {
        let mut play = report("2", 5000, Value::Null);
        play["playId"] = json!(format!("{}{n}", &PLAY[..35]));
        play["context"] = context.clone();
        play["origin"] = json!(origin);
        assert_eq!(post(&app, TOKEN, json!([play])).await, StatusCode::NO_CONTENT);
    }
    let stored = db
        .read(|conn| {
            conn.prepare("SELECT context, origin FROM plays ORDER BY origin")?
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()
        })
        .await
        .unwrap();
    let context = context.to_string();
    assert_eq!(stored, [(context.clone(), "context".to_owned()), (context, "manual".to_owned())]);
}
