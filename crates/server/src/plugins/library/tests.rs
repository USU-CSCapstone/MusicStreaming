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
