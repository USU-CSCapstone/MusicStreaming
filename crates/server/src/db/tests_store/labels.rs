//! Album labels, resolved from the labels each track's tags carry (`design/database.md` §3).

use std::fs::File;
use std::path::Path;
use std::time::{Duration, SystemTime};

use jewelcase_scanner::testing;
use lofty::tag::ItemKey;

use super::recompute::check;
use super::{make, scalar, tagged};

/// The tags of track `number` of album X, with `label` as its label tag when it has one.
fn tags(number: &'static str, label: Option<&'static str>) -> Vec<(ItemKey, &'static str)> {
    let mut items = vec![
        (ItemKey::TrackArtist, "A"),
        (ItemKey::AlbumTitle, "X"),
        (ItemKey::TrackNumber, number),
    ];
    items.extend(label.map(|label| (ItemKey::Label, label)));
    items
}

/// Retags the file at `rel` under `root` with `items`, dated later so the next scan reads it.
fn retag(root: &Path, rel: &str, items: &[(ItemKey, &str)]) {
    let path = root.join(rel);
    testing::tag_wav_with(&path, items);
    let later = SystemTime::now() + Duration::from_secs(5);
    File::open(&path).unwrap().set_modified(later).unwrap();
}

/// An album's labels are those of its first track that has any, in track order, as tagged; a
/// track losing its labels hands the album the next one's, and none leaves it none.
#[test]
fn an_albums_labels_are_its_first_labelled_tracks() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(&root, "A/X/1.wav", &tags("1", None));
    tagged(&root, "A/X/3.wav", &tags("3", Some("Matador")));
    tagged(&root, "A/X/2.wav", &tags("2", Some("Sub Pop;Domino")));
    let store = make(&root);
    let lib = testing::library(&root);
    let labels = |step: &str| {
        testing::scan_library(&store, &lib);
        check(&store, step);
        scalar::<String>(&store, "SELECT labels FROM albums").unwrap()
    };
    assert_eq!(labels("the first scan"), r#"["Sub Pop","Domino"]"#);

    retag(&root, "A/X/2.wav", &tags("2", None));
    assert_eq!(labels("track 2 losing its labels"), r#"["Matador"]"#);

    retag(&root, "A/X/3.wav", &tags("3", None));
    assert_eq!(labels("track 3 losing its labels"), "[]");
}
