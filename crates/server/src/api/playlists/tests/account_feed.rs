//! The account feed: every playlist write records its change in its own transaction.

use super::edit::{edit, send, track};
use super::*;

/// A feed row.
type Row = (i64, String, i64, i64, String, i64);

/// The account feed as `(user, kind, playlist, item key, op, seq)` in seq order.
async fn feed(db: &Database) -> Vec<Row> {
    db.read(|conn| {
        conn.prepare(
            "SELECT user_id, kind, entity_id, item_key, op, seq FROM account_changes ORDER BY seq",
        )?
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })?
        .collect()
    })
    .await
    .unwrap()
}

/// The rows of `kind` in the feed, as `(item key, op)`.
fn of(feed: &[Row], kind: &str) -> Vec<(i64, String)> {
    feed.iter().filter(|r| r.1 == kind).map(|r| (r.3, r.4.clone())).collect()
}

/// The numeric ID of a playlist representation.
fn id(playlist: &Value) -> i64 {
    playlist["id"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn each_playlist_write_records_its_changes_in_its_transaction() {
    let (_temp, db, app) = catalog().await;
    let made = create(&app, json!({ "title": "Mix", "items": [track(1), track(2)] })).await;
    let (pid, sid) = (id(&made), made["id"].as_str().unwrap());
    let keys: Vec<i64> = db
        .read(move |conn| {
            conn.prepare("SELECT key FROM playlist_items WHERE playlist_id = ?1 ORDER BY key")?
                .query_map([pid], |row| row.get(0))?
                .collect()
        })
        .await
        .unwrap();
    let after_create = feed(&db).await;
    assert!(after_create.iter().all(|r| r.0 == 1 && r.2 == pid));
    assert_eq!(of(&after_create, "playlist"), [(0, "upsert".to_owned())]);
    let upserts: Vec<_> = keys.iter().map(|&k| (k, "upsert".to_owned())).collect();
    assert_eq!(of(&after_create, "playlistItem"), upserts);

    // Details alone touch only the playlist's row, which moves to the end of the feed.
    let (status, _) = edit(&app, sid, json!({ "baseVersion": 1, "title": "Renamed" })).await;
    assert_eq!(status, StatusCode::OK);
    let renamed = feed(&db).await;
    assert_eq!(renamed.len(), after_create.len());
    let (old, new) = (after_create.iter().find(|r| r.1 == "playlist"), renamed.last());
    let (old, new) = (old.unwrap(), new.unwrap());
    assert_eq!(new.1, "playlist");
    assert!(new.5 > old.5 && new.5 > after_create.last().unwrap().5, "moved to the end");

    // A stale edit records nothing.
    let (status, _) = edit(&app, sid, json!({ "baseVersion": 1, "title": "Stale" })).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(feed(&db).await, renamed);

    // Removing the first item leaves its tombstone; adding one gives a new key.
    let ops = json!([{ "op": "remove", "positions": [0] }, { "op": "add", "items": [track(11)] }]);
    let (status, _) = edit(&app, sid, json!({ "baseVersion": 2, "ops": ops })).await;
    assert_eq!(status, StatusCode::OK);
    let items = of(&feed(&db).await, "playlistItem");
    assert_eq!(items.len(), 3);
    assert!(items.contains(&(keys[0], "delete".to_owned())));
    assert!(items.contains(&(keys[1], "upsert".to_owned())));
    assert_eq!(items.iter().filter(|r| r.1 == "upsert").count(), 2);
}

#[tokio::test]
async fn a_second_edit_of_an_item_replaces_its_earlier_row() {
    let (_temp, db, app) = catalog().await;
    let made = create(&app, json!({ "title": "Mix", "items": [track(1), track(2)] })).await;
    let sid = made["id"].as_str().unwrap();
    // Moved twice: the item's key changes each time, and each key has one row.
    for (version, at) in [(1, 0), (2, 1)] {
        let ops = json!([{ "op": "move", "positions": [if at == 0 { 1 } else { 0 }], "at": at }]);
        let (status, _) = edit(&app, sid, json!({ "baseVersion": version, "ops": ops })).await;
        assert_eq!(status, StatusCode::OK);
    }
    let all = feed(&db).await;
    let mut seen = all.iter().map(|r| (&r.1, r.2, r.3)).collect::<Vec<_>>();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), all.len(), "one row for each thing: {all:?}");
    assert_eq!(of(&all, "playlist").len(), 1);
}

/// The keys of playlist `pid`'s items, in order.
async fn live(db: &Database, pid: i64) -> Vec<i64> {
    db.read(move |conn| {
        conn.prepare("SELECT key FROM playlist_items WHERE playlist_id = ?1 ORDER BY key")?
            .query_map([pid], |row| row.get(0))?
            .collect()
    })
    .await
    .unwrap()
}

/// Asserts each item row of the feed is an upsert exactly when its key is live.
fn agrees(feed: &[Row], live: &[i64]) {
    for (key, op) in of(feed, "playlistItem") {
        assert_eq!(op == "upsert", live.contains(&key), "{key} {op} live {live:?} feed {feed:?}");
    }
}

#[tokio::test]
async fn a_key_deleted_and_taken_again_in_one_write_ends_as_an_upsert() {
    let (_temp, db, app) = catalog().await;
    let made =
        create(&app, json!({ "title": "P", "items": [track(1), track(2), track(12)] })).await;
    let (pid, sid) = (id(&made), made["id"].as_str().unwrap());
    // The new item takes the midpoint, the key of the item just removed.
    let ops = json!([
        { "op": "remove", "positions": [1] },
        { "op": "add", "items": [track(11)], "at": 1 }
    ]);
    let (status, _) = edit(&app, sid, json!({ "baseVersion": 1, "ops": ops })).await;
    assert_eq!(status, StatusCode::OK);
    let keys = live(&db, pid).await;
    assert!(keys.contains(&(2 * super::KEY_GAP)), "the key is reused: {keys:?}");
    let feed = feed(&db).await;
    assert_eq!(of(&feed, "playlistItem").len(), 3);
    agrees(&feed, &keys);
}

#[tokio::test]
async fn a_renumber_tombstones_the_keys_it_drops_and_keeps_the_ones_it_reuses() {
    let (_temp, db, app) = catalog().await;
    let made =
        create(&app, json!({ "title": "P", "items": [track(2), track(13), track(12)] })).await;
    let (pid, sid) = (id(&made), made["id"].as_str().unwrap());
    // Keys G, G + 1 and G + 2: no room between two, and the renumber to G, 2G, 3G reuses only G.
    let gap = super::KEY_GAP;
    db.write(move |tx| tx.execute("UPDATE playlist_items SET key = ?1 + key / ?1 - 1", [gap]))
        .await
        .unwrap();
    let ops = json!([{ "op": "move", "positions": [2], "at": 1 }]);
    let (status, _) = edit(&app, sid, json!({ "baseVersion": 1, "ops": ops })).await;
    assert_eq!(status, StatusCode::OK);
    let keys = live(&db, pid).await;
    assert_eq!(keys, [gap, 2 * gap, 3 * gap]);
    let feed = feed(&db).await;
    let items = of(&feed, "playlistItem");
    assert!(items.contains(&(gap + 1, "delete".to_owned())), "{items:?}");
    assert!(items.contains(&(gap + 2, "delete".to_owned())), "{items:?}");
    agrees(&feed, &keys);
}

#[tokio::test]
async fn a_deleted_playlist_leaves_one_tombstone_and_drops_its_item_rows() {
    let (_temp, db, app) = catalog().await;
    let doomed = create(&app, json!({ "title": "Doomed", "items": [track(1), track(2)] })).await;
    let kept = create(&app, json!({ "title": "Kept", "items": [track(1)] })).await;
    let sid = doomed["id"].as_str().unwrap();
    edit(&app, sid, json!({ "baseVersion": 1, "ops": [{ "op": "remove", "positions": [0] }] }))
        .await;
    let (status, _) = send(&app, TOKEN, "DELETE", &format!("{URI}/{sid}"), None, Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let all = feed(&db).await;
    let doomed_rows: Vec<_> = all.iter().filter(|r| r.2 == id(&doomed)).collect();
    assert_eq!(doomed_rows.len(), 1, "{all:?}");
    assert_eq!((doomed_rows[0].1.as_str(), doomed_rows[0].4.as_str()), ("playlist", "delete"));
    assert_eq!(all.iter().filter(|r| r.2 == id(&kept)).count(), 2, "{all:?}");
    // Gone twice, recorded once: no row is new.
    let (status, _) = send(&app, TOKEN, "DELETE", &format!("{URI}/{sid}"), None, Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(feed(&db).await, all);
}
