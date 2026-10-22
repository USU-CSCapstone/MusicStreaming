//! Scenarios for how tracks group into albums (`requirements/albums.md` §1).

use lofty::tag::ItemKey;

use super::{Factory, InspectStore, library, live, scan_library, tag_wav_with, write_small_wav};

/// A file with no album-artist tag gives its album its primary artist: a guest on one track
/// neither splits the album nor joins its artists, an untagged album whose tracks have different
/// primary artists splits by primary artist, a compilation's album artist is unknown whether its
/// flag or its release type says so, and a tagged album artist is taken as tagged.
pub fn an_absent_album_artist_is_the_primary_artist<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let released = |artist| {
        [
            (ItemKey::TrackArtist, artist),
            (ItemKey::AlbumTitle, "R"),
            (ItemKey::MusicBrainzReleaseType, "compilation"),
        ]
    };
    let files: [(&str, &[(ItemKey, &str)]); 7] = [
        ("A/X/1.wav", &[(ItemKey::TrackArtist, "A"), (ItemKey::AlbumTitle, "X")]),
        ("A/X/2.wav", &[(ItemKey::TrackArtist, "A;B"), (ItemKey::AlbumTitle, "X")]),
        ("C/X/1.wav", &[(ItemKey::TrackArtist, "C;A"), (ItemKey::AlbumTitle, "X")]),
        (
            "V/Y/1.wav",
            &[
                (ItemKey::TrackArtist, "D;E"),
                (ItemKey::AlbumTitle, "Y"),
                (ItemKey::FlagCompilation, "1"),
            ],
        ),
        ("W/R/1.wav", &released("G")),
        ("W/R/2.wav", &released("H")),
        (
            "Z/Z/1.wav",
            &[
                (ItemKey::TrackArtist, "A;B"),
                (ItemKey::AlbumArtist, "B;F"),
                (ItemKey::AlbumTitle, "Z"),
            ],
        ),
    ];
    for (rel, items) in files {
        let path = root.join(rel);
        write_small_wav(&path);
        tag_wav_with(&path, items);
    }
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);

    let album_artists: Vec<(String, Vec<String>)> = live(&store, &lib)
        .into_iter()
        .map(|t| (t.path.strip_prefix(&root).unwrap().display().to_string(), t.album_artists))
        .collect();
    let want = |rel: &str, artists: &[&str]| {
        (rel.to_owned(), artists.iter().map(|&a| a.to_owned()).collect::<Vec<_>>())
    };
    assert_eq!(
        album_artists,
        [
            want("A/X/1.wav", &["A"]),
            want("A/X/2.wav", &["A"]),
            want("C/X/1.wav", &["C"]),
            want("V/Y/1.wav", &[]),
            want("W/R/1.wav", &[]),
            want("W/R/2.wav", &[]),
            want("Z/Z/1.wav", &["B", "F"]),
        ]
    );
}
