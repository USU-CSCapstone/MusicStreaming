use jewelcase_plugins::Library;

use super::super::testing::database;
use super::RunLibrary;

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
async fn gets_albums_and_artists_by_id_with_what_they_have() {
    let (_temp, db) = database().await;
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO images (id, library_id, hash, format, width, height, root_id, path, embedded)
             SELECT 1, 1, x'01', 'jpeg', 600, 600, id, 'Signal/cover.jpg', 0
             FROM library_roots WHERE library_id = 1;
             UPDATE albums SET image_id = 1 WHERE id = 101;
             UPDATE artists SET biography = 'From the coast.' WHERE id = 301;",
        )
    })
    .await
    .unwrap();
    let mut library = RunLibrary::new(db, "p", 1);
    let albums = library.get_albums(vec![201, 102, 101, 999]).await.unwrap();
    assert_eq!(ids(&albums, |a| a.id), [101], "not another library's, nor the empty one");
    assert!(albums[0].has_artwork);
    assert!(!library.albums(None, 10).await.unwrap().is_empty());

    let artists = library.get_artists(vec![999, 301]).await.unwrap();
    assert_eq!(ids(&artists, |a| a.id), [301]);
    assert_eq!((artists[0].has_image, artists[0].has_biography), (false, true));
}

#[tokio::test]
async fn reads_an_albums_tracks_and_an_artists_albums() {
    let (_temp, db) = database().await;
    // An artist who only appears on a track, and owns no album (`requirements/artists.md` §2).
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO artists (id, library_id, name, name_key, sort_key, album_count,
                                  track_count, added_at, updated_at)
             VALUES (302, 1, 'Guest', 'guest', x'67', 0, 1, 0, 0);
             INSERT INTO track_artists (library_id, track_id, position, artist_id, artist_name)
             VALUES (1, 12, 1, 302, 'Guest');",
        )
    })
    .await
    .unwrap();
    let mut library = RunLibrary::new(db, "p", 1);
    let tracks = library.album_tracks(101).await.unwrap();
    assert_eq!(ids(&tracks, |t| t.id), [11, 12], "by number, and not 13, which is missing");
    assert!(library.album_tracks(201).await.unwrap().is_empty(), "another library's");

    assert_eq!(ids(&library.artist_albums(301).await.unwrap(), |a| a.id), [101]);
    assert!(library.artist_albums(302).await.unwrap().is_empty(), "an appearance only");
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
