//! Album labels, resolved from the labels each track's tags carry (`design/database.md` §3).

use jewelcase_scanner::testing::{self, ItemKey};

use super::recompute::check;
use super::{make, retag, scalar, tagged};

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

/// An album's labels are the set most of its tracks carry, as tagged, ties broken by the set's
/// stored bytes; a track losing its labels can hand the album another set, and none leaves it
/// none.
#[test]
fn an_albums_labels_are_the_set_most_of_its_tracks_carry() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(&root, "A/X/1.wav", &tags("1", Some("Sub Pop;Domino")));
    tagged(&root, "A/X/2.wav", &tags("2", Some("Sub Pop;Domino")));
    tagged(&root, "A/X/3.wav", &tags("3", Some("Matador")));
    tagged(&root, "A/X/4.wav", &tags("4", None));
    let store = make(&root);
    let lib = testing::library(&root);
    let labels = |step: &str| {
        testing::scan_library(&store, &lib);
        check(&store, step);
        scalar::<String>(&store, "SELECT labels FROM albums").unwrap()
    };
    assert_eq!(labels("the first scan"), r#"["Sub Pop","Domino"]"#);

    retag(&root, "A/X/2.wav", &tags("2", None));
    assert_eq!(labels("a tie"), r#"["Matador"]"#, "the set that sorts first");

    retag(&root, "A/X/3.wav", &tags("3", None));
    assert_eq!(labels("track 3 losing its labels"), r#"["Sub Pop","Domino"]"#);

    retag(&root, "A/X/1.wav", &tags("1", None));
    assert_eq!(labels("the last labelled track losing its labels"), "[]");
}
