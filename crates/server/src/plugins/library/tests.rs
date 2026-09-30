use std::path::Path;
use std::sync::Arc;

use jewelcase_plugins::Library;

use super::RunLibrary;
use crate::db::{Database, libraries};

/// Library 1 holds tracks 11, 12, and 13 (13 missing since a scan) on album 101 by artist 201;
/// library 2 holds track 21. The plugin `p` is installed.
async fn database() -> (tempfile::TempDir, Arc<Database>) {
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

fn ids<T>(items: &[T], id: fn(&T) -> u64) -> Vec<u64> {
    items.iter().map(id).collect()
}

#[tokio::test]
async fn pages_the_librarys_present_tracks_by_id() {
    let (_temp, db) = database().await;
    let mut library = RunLibrary::new(db, "p", 1);
    let first = library.tracks(None, 1).await.unwrap();
    assert_eq!(ids(&first, |t| t.id), [11]);
    let track = &first[0];
    assert_eq!((track.title.as_str(), track.album.as_deref()), ("Track 11", Some("Signal")));
    assert_eq!(
        (track.artists.as_slice(), track.path.as_str()),
        (["Aurora Lane".to_owned()].as_slice(), "Signal/11.flac")
    );
    assert_eq!((track.duration_ms, track.track_number), (180_000, Some(1)));
    assert_eq!(ids(&library.tracks(Some(11), 10).await.unwrap(), |t| t.id), [12], "13 is missing");
    assert!(library.tracks(Some(12), 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn gets_tracks_in_the_order_asked_and_only_from_this_library() {
    let (_temp, db) = database().await;
    let mut library = RunLibrary::new(db, "p", 1);
    let got = library.get_tracks(vec![12, 21, 99, 11, 13]).await.unwrap();
    assert_eq!(ids(&got, |t| t.id), [12, 11]);
}

#[tokio::test]
async fn pages_albums_and_artists_with_music() {
    let (_temp, db) = database().await;
    let mut library = RunLibrary::new(db, "p", 1);
    let albums = library.albums(None, 10).await.unwrap();
    assert_eq!(ids(&albums, |a| a.id), [101], "not the empty album, nor another library's");
    assert_eq!(
        (albums[0].artists.as_slice(), albums[0].track_count),
        (["Aurora Lane".to_owned()].as_slice(), 2)
    );
    let artists = library.artists(None, 10).await.unwrap();
    assert_eq!(
        (ids(&artists, |a| a.id), artists[0].name.as_deref()),
        (vec![301], Some("Aurora Lane"))
    );
}

#[tokio::test]
async fn keeps_state_per_plugin_and_library_until_uninstalled() {
    let (_temp, db) = database().await;
    let mut here = RunLibrary::new(db.clone(), "p", 1);
    let mut there = RunLibrary::new(db.clone(), "p", 2);
    here.state_set("cursor".into(), b"12".to_vec()).await.unwrap();
    here.state_set("cursor".into(), b"13".to_vec()).await.unwrap();
    assert_eq!(here.state_get("cursor".into()).await.unwrap().as_deref(), Some(b"13".as_slice()));
    assert_eq!(there.state_get("cursor".into()).await.unwrap(), None);
    here.state_delete("cursor".into()).await.unwrap();
    assert_eq!(here.state_get("cursor".into()).await.unwrap(), None);

    here.state_set("kept".into(), b"x".to_vec()).await.unwrap();
    db.write(|tx| tx.execute("DELETE FROM plugins WHERE id = 'p'", [])).await.unwrap();
    assert_eq!(here.state_get("kept".into()).await.unwrap(), None);
}
