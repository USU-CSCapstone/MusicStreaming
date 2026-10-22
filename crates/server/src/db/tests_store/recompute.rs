//! Every derived value the triggers and the scanner keep, as stored and as recomputed from the
//! tracks and their credits, and the change feed's record of what changed.

use std::collections::BTreeMap;

use jewelcase_core::sort_key;
use rusqlite::types::Value;
use rusqlite::{Connection, Result};

use crate::db::store::SqliteStore;

/// Each derived value as stored, beside the same value recomputed from scratch, as two queries
/// returning the same rows in the same order.
const CHECKS: &[(&str, &str, &str)] = &[
    (
        "library totals",
        "SELECT id, track_count, album_count, artist_count, duration_us FROM libraries ORDER BY id",
        "SELECT l.id, (SELECT COUNT(*) FROM tracks WHERE library_id = l.id), \
         (SELECT COUNT(DISTINCT album_id) FROM tracks WHERE library_id = l.id), \
         (SELECT COUNT(*) FROM (SELECT artist_id FROM track_artists WHERE library_id = l.id \
          UNION SELECT artist_id FROM track_album_artists WHERE library_id = l.id)), \
         (SELECT COALESCE(SUM(duration_us), 0) FROM tracks WHERE library_id = l.id) \
         FROM libraries l ORDER BY l.id",
    ),
    (
        "artists",
        "SELECT id, name, album_count, track_count, appearance_count, available_track_count \
         FROM artists ORDER BY id",
        "WITH firsts AS (SELECT MIN(id) AS id FROM tracks GROUP BY album_id), \
         listed AS (SELECT t.album_id, tam.artist_id FROM track_album_artists tam \
                    JOIN tracks t ON t.id = tam.track_id WHERE t.id IN firsts) \
         SELECT a.id, (SELECT artist_name FROM (SELECT artist_name FROM track_artists WHERE artist_id = a.id \
                       UNION ALL SELECT artist_name FROM track_album_artists WHERE artist_id = a.id) \
                       WHERE artist_name IS NOT NULL GROUP BY artist_name \
                       ORDER BY COUNT(*) DESC, artist_name LIMIT 1), \
         (SELECT COUNT(*) FROM listed WHERE artist_id = a.id), \
         (SELECT COUNT(*) FROM track_artists WHERE artist_id = a.id), \
         (SELECT COUNT(*) FROM track_artists ta JOIN tracks t ON t.id = ta.track_id WHERE ta.artist_id = a.id \
          AND NOT EXISTS (SELECT 1 FROM listed l WHERE l.album_id = t.album_id AND l.artist_id = a.id)), \
         (SELECT COUNT(*) FROM track_artists ta JOIN tracks t ON t.id = ta.track_id \
          WHERE ta.artist_id = a.id AND t.missing_since IS NULL) \
         FROM artists a WHERE a.id IN (SELECT artist_id FROM track_artists \
                                       UNION SELECT artist_id FROM track_album_artists) ORDER BY a.id",
    ),
    (
        "albums",
        "SELECT id, title, track_count, available_track_count, duration_us, disc_total, track_total, \
         disc_count FROM albums ORDER BY id",
        "SELECT album_id, (SELECT album_title FROM tracks WHERE album_id = t.album_id AND album_title IS NOT NULL \
                           GROUP BY album_title ORDER BY COUNT(*) DESC, album_title LIMIT 1), \
         COUNT(*), SUM(missing_since IS NULL), SUM(duration_us), MAX(disc_total), \
         (SELECT SUM(total) FROM (SELECT MAX(track_total) AS total FROM tracks \
                                  WHERE album_id = t.album_id GROUP BY disc_number)), \
         COUNT(DISTINCT disc_number) FROM tracks t GROUP BY album_id ORDER BY album_id",
    ),
    (
        "album artist sort keys",
        "SELECT id, artist_sort_key FROM albums ORDER BY id",
        "SELECT al.id, (SELECT a.sort_key FROM album_artists aa JOIN artists a ON a.id = aa.artist_id \
                        WHERE aa.album_id = al.id AND aa.position = 0) \
         FROM albums al ORDER BY al.id",
    ),
    (
        "album art",
        "SELECT id, image_id FROM albums ORDER BY id",
        "SELECT DISTINCT album_id, (SELECT image_id FROM tracks WHERE album_id = t.album_id \
                                    AND image_id IS NOT NULL ORDER BY missing_since IS NOT NULL, \
                                    image_embedded DESC, disc_number, \
                                    ifnull(track_number, 9223372036854775807), path, id LIMIT 1) \
         FROM tracks t ORDER BY album_id",
    ),
    (
        "album labels",
        "SELECT id, labels FROM albums ORDER BY id",
        "SELECT DISTINCT album_id, ifnull((SELECT labels FROM tracks WHERE album_id = t.album_id \
                                           AND labels <> '[]' ORDER BY missing_since IS NOT NULL, \
                                           disc_number, ifnull(track_number, 9223372036854775807), \
                                           path, id LIMIT 1), '[]') \
         FROM tracks t ORDER BY album_id",
    ),
    (
        "artist images and biographies",
        "SELECT id, image_id, biography FROM artists ORDER BY id",
        "WITH credited AS (SELECT c.artist_id, t.* FROM track_album_artists c JOIN tracks t ON t.id = c.track_id) \
         SELECT a.id, \
         (SELECT artist_image_id FROM credited WHERE artist_id = a.id AND artist_image_id IS NOT NULL \
          AND a.name IS NOT NULL ORDER BY missing_since IS NOT NULL, disc_number, \
          ifnull(track_number, 9223372036854775807), path, id LIMIT 1), \
         (SELECT b.text FROM credited JOIN biographies b ON b.id = credited.biography_id \
          WHERE credited.artist_id = a.id AND a.name IS NOT NULL ORDER BY missing_since IS NOT NULL, \
          disc_number, ifnull(track_number, 9223372036854775807), path, credited.id LIMIT 1) \
         FROM artists a ORDER BY a.id",
    ),
    (
        "album artists",
        "SELECT album_id, position, artist_id FROM album_artists ORDER BY album_id, position",
        "SELECT t.album_id, tam.position, tam.artist_id FROM track_album_artists tam \
         JOIN tracks t ON t.id = tam.track_id \
         WHERE t.id IN (SELECT MIN(id) FROM tracks GROUP BY album_id) ORDER BY 1, 2",
    ),
    (
        "album discs",
        "SELECT album_id, disc_number, track_count, track_total FROM album_discs ORDER BY 1, 2",
        "SELECT album_id, disc_number, COUNT(*), MAX(track_total) FROM tracks GROUP BY 1, 2 ORDER BY 1, 2",
    ),
    (
        "album tags",
        "SELECT library_id, album_id, tag_id, track_count FROM album_tags ORDER BY 2, 3",
        "SELECT t.library_id, t.album_id, tt.tag_id, COUNT(*) FROM track_tags tt \
         JOIN tracks t ON t.id = tt.track_id GROUP BY 2, 3 ORDER BY 2, 3",
    ),
    (
        "artist tags",
        "SELECT library_id, artist_id, tag_id, track_count FROM artist_tags ORDER BY 2, 3",
        "SELECT ta.library_id, ta.artist_id, tt.tag_id, COUNT(*) FROM track_tags tt \
         JOIN track_artists ta ON ta.track_id = tt.track_id GROUP BY 2, 3 ORDER BY 2, 3",
    ),
    (
        "tags",
        "SELECT id, name, track_count, album_count, artist_count FROM tags ORDER BY id",
        "SELECT g.id, (SELECT tag_name FROM track_tags WHERE tag_id = g.id GROUP BY tag_name \
                       ORDER BY COUNT(*) DESC, tag_name LIMIT 1), \
         (SELECT COUNT(*) FROM track_tags WHERE tag_id = g.id), \
         (SELECT COUNT(DISTINCT t.album_id) FROM track_tags tt JOIN tracks t ON t.id = tt.track_id \
          WHERE tt.tag_id = g.id), \
         (SELECT COUNT(DISTINCT ta.artist_id) FROM track_tags tt \
          JOIN track_artists ta ON ta.track_id = tt.track_id WHERE tt.tag_id = g.id) \
         FROM tags g WHERE g.id IN (SELECT tag_id FROM track_tags) ORDER BY g.id",
    ),
    (
        "artist spellings",
        "SELECT library_id, artist_id, name, uses FROM artist_spellings ORDER BY 2, 3",
        "SELECT library_id, artist_id, artist_name, COUNT(*) FROM (SELECT * FROM track_artists \
         UNION ALL SELECT * FROM track_album_artists) WHERE artist_name IS NOT NULL \
         GROUP BY 2, 3 ORDER BY 2, 3",
    ),
    (
        "artist sort keys",
        "SELECT library_id, artist_id, sort_key, uses FROM artist_sort_keys ORDER BY 2, 3",
        "SELECT ta.library_id, ta.artist_id, t.artist_sort_key, COUNT(*) FROM track_artists ta \
         JOIN tracks t ON t.id = ta.track_id WHERE ta.position = 0 GROUP BY 2, 3 ORDER BY 2, 3",
    ),
    (
        "tag spellings",
        "SELECT library_id, tag_id, name, uses FROM tag_spellings ORDER BY 2, 3",
        "SELECT library_id, tag_id, tag_name, COUNT(*) FROM track_tags GROUP BY 2, 3 ORDER BY 2, 3",
    ),
    (
        "sort keys",
        // An artist's most-used tagged sort key unless it is empty, else a name, which `check`
        // turns into its sort key; names are checked above.
        "SELECT 'artist', id, sort_key FROM artists UNION ALL SELECT 'tag', id, sort_key FROM tags ORDER BY 1, 2",
        "SELECT 'artist', a.id, COALESCE(NULLIF((SELECT t.artist_sort_key FROM track_artists ta \
          JOIN tracks t ON t.id = ta.track_id WHERE ta.artist_id = a.id AND ta.position = 0 \
          GROUP BY t.artist_sort_key ORDER BY COUNT(*) DESC, t.artist_sort_key LIMIT 1), x''), a.name) \
         FROM artists a UNION ALL SELECT 'tag', id, name FROM tags ORDER BY 1, 2",
    ),
];

/// The rows `sql` returns.
fn rows(conn: &Connection, sql: &str) -> Result<Vec<Vec<Value>>> {
    let mut stmt = conn.prepare(sql)?;
    let width = stmt.column_count();
    let rows = stmt.query_map([], |r| (0..width).map(|i| r.get::<_, Value>(i)).collect())?;
    rows.collect()
}

/// Asserts that every stored derived value equals its recompute, after `step`.
pub fn check(store: &SqliteStore, step: &str) {
    for (what, stored, scratch) in CHECKS {
        let (stored, mut scratch) = store
            .db()
            .read_blocking(|conn| Ok((rows(conn, stored)?, rows(conn, scratch)?)))
            .unwrap();
        if *what == "sort keys" {
            for row in &mut scratch {
                let name = match &row[2] {
                    Value::Blob(_) => continue,
                    Value::Text(name) => name.clone(),
                    _ => String::new(),
                };
                row[2] = Value::Blob(sort_key(&name, None).into_bytes());
            }
        }
        assert_eq!(stored, scratch, "{what} after {step}");
    }
}

/// The albums, artists and tags, each row by its kind and ID, and the feed's last position.
pub struct Snapshot {
    rows: BTreeMap<(&'static str, i64), Vec<Value>>,
    seq: i64,
}

/// What the catalog holds now, for [`assert_fed`] to compare against.
pub fn snapshot(store: &SqliteStore) -> Snapshot {
    store
        .db()
        .read_blocking(|conn| {
            let mut rows = BTreeMap::new();
            for (kind, table) in [("album", "albums"), ("artist", "artists"), ("tag", "tags")] {
                for row in self::rows(conn, &format!("SELECT * FROM {table}"))? {
                    let Value::Integer(id) = row[0] else { panic!("{table} ids are integers") };
                    rows.insert((kind, id), row);
                }
            }
            let seq =
                conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM library_changes", [], |r| {
                    r.get(0)
                })?;
            Ok(Snapshot { rows, seq })
        })
        .unwrap()
}

/// Asserts that every album, artist and tag added, removed or changed since `before` has a
/// change-feed row written after it, after `step`.
pub fn assert_fed(store: &SqliteStore, before: &Snapshot, step: &str) {
    let (after, seq) = (snapshot(store), before.seq);
    let keys = before.rows.keys().chain(after.rows.keys());
    for &(kind, id) in keys.filter(|k| before.rows.get(k) != after.rows.get(k)) {
        let fed: bool = store
            .db()
            .read_blocking(move |conn| {
                conn.query_row(
                    "SELECT EXISTS (SELECT 1 FROM library_changes \
                     WHERE entity_type = ?1 AND entity_id = ?2 AND seq > ?3)",
                    rusqlite::params![kind, id, seq],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert!(fed, "{kind} {id} changed with no feed row after {step}");
    }
}
