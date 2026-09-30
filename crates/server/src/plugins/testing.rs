//! A small catalog for the plugins' tests.

use std::path::Path;
use std::sync::Arc;

use crate::db::{Database, libraries};

/// Library 1 holds tracks 11, 12, and 13 (13 missing since a scan) on album 101 by artist 201;
/// library 2 holds track 21. The plugin `p` is installed.
pub async fn database() -> (tempfile::TempDir, Arc<Database>) {
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&temp.path().join("jewelcase.db")).unwrap());
    db.write(|tx| {
        libraries::create(tx, Some(1), "Music", &[Path::new("/music")], &[])?;
        libraries::create(tx, Some(2), "Other", &[Path::new("/other")], &[])?;
        tx.execute_batch(
            "INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key,
                                 artist_sort_key, track_count, added_at, updated_at)
             VALUES (101, 1, 'Signal', 'a', '', x'61', x'', 2, 0, 0),
                    (102, 1, 'Empty', 'b', '', x'62', x'', 0, 0, 0),
                    (201, 2, 'Elsewhere', 'a', '', x'61', x'', 1, 0, 0);
             INSERT INTO artists (id, library_id, name, name_key, sort_key, album_count,
                                  track_count, added_at, updated_at)
             VALUES (301, 1, 'Aurora Lane', 'aurora lane', x'61', 1, 2, 0, 0);
             INSERT INTO album_artists VALUES (1, 101, 0, 301);
             WITH t (id, library_id, album_id, missing) AS
                 (VALUES (11, 1, 101, NULL), (12, 1, 101, NULL), (13, 1, 101, 5), (21, 2, 201, NULL))
             INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size, file_mtime,
                                 missing_since, title, sort_key, artist_sort_key, album_sort_key,
                                 track_number, codec, container, lossless, sample_rate_hz,
                                 channels, duration_us, added_at, updated_at)
             SELECT t.id, t.library_id, t.album_id, r.id, 'Signal/' || t.id || '.flac', 10, 0,
                    t.missing, 'Track ' || t.id, x'61', x'', x'61', t.id % 10, 'flac', 'flac', 1,
                    44100, 2, 180000000, 0, 0
             FROM t JOIN library_roots r ON r.library_id = t.library_id;
             INSERT INTO track_artists (library_id, track_id, position, artist_id, artist_name)
             VALUES (1, 11, 0, 301, 'Aurora Lane');
             INSERT INTO plugins (id, manifest, installed_at, updated_at) VALUES ('p', '{}', 0, 0);",
        )
    })
    .await
    .unwrap();
    (temp, db)
}
