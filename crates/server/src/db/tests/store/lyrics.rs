//! Lyrics are stored in one form: timed lines when any line carried a timestamp, the text
//! otherwise, never both (`requirements/tracks.md` §5).

use jewelcase_scanner::testing::{self, ItemKey};

use super::{make, retag, scalar, tagged};
use crate::db::store::SqliteStore;

/// Each track's title, lyrics kind, plain text and timed lines as stored, by title.
fn stored(store: &SqliteStore) -> Vec<(String, String, Option<String>, Option<String>)> {
    store
        .db()
        .read_blocking(|conn| {
            conn.prepare(
                "SELECT t.title, t.lyrics_kind, l.plain, l.synced \
                 FROM tracks t LEFT JOIN track_lyrics l ON l.track_id = t.id ORDER BY t.title",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect()
        })
        .unwrap()
}

#[test]
fn timed_lyrics_store_their_lines_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let lrc = "[ti:Song]\n[00:01.00]first\n[00:02.50]second";
    tagged(&root, "a/1.wav", &[(ItemKey::TrackTitle, "1"), (ItemKey::UnsyncLyrics, lrc)]);
    tagged(&root, "a/2.wav", &[(ItemKey::TrackTitle, "2")]);
    std::fs::write(root.join("a/2.lrc"), lrc).unwrap();
    // A stamp `[00:60.00]` looks like, but is not, a time: the text stays plain.
    tagged(&root, "a/3.wav", &[(ItemKey::TrackTitle, "3")]);
    std::fs::write(root.join("a/3.lrc"), "[00:60.00]never").unwrap();
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));

    let lines = r#"[{"startMs":1000,"text":"first"},{"startMs":2500,"text":"second"}]"#;
    let synced = |title: &str| (title.into(), "synced".into(), None, Some(lines.into()));
    assert_eq!(
        stored(&store),
        [
            synced("1"),
            synced("2"),
            ("3".into(), "plain".into(), Some("[00:60.00]never".into()), None)
        ]
    );
}

#[test]
fn plain_lyrics_store_without_a_byte_order_mark() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let text = "\u{FEFF}Hello\nworld\n";
    tagged(&root, "a/1.wav", &[(ItemKey::TrackTitle, "1"), (ItemKey::UnsyncLyrics, text)]);
    tagged(&root, "a/2.wav", &[(ItemKey::TrackTitle, "2")]);
    std::fs::write(root.join("a/2.txt"), text).unwrap();
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));

    let plain = |title: &str| (title.into(), "plain".into(), Some("Hello\nworld".into()), None);
    assert_eq!(stored(&store), [plain("1"), plain("2")]);
}

#[test]
fn a_track_cannot_store_both_forms() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(&root, "a/1.wav", &[(ItemKey::TrackTitle, "1")]);
    std::fs::write(root.join("a/1.lrc"), "[00:01.00]first").unwrap();
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));

    let error = store
        .db()
        .write_blocking(|tx| tx.execute("UPDATE track_lyrics SET plain = 'first'", []))
        .unwrap_err();
    assert!(error.to_string().contains("CHECK constraint failed"), "{error}");
}

#[test]
fn an_embedded_tag_holding_only_a_byte_order_mark_leaves_the_lyrics_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(&root, "a/1.wav", &[(ItemKey::TrackTitle, "1"), (ItemKey::UnsyncLyrics, "\u{FEFF}")]);
    std::fs::write(root.join("a/1.lrc"), "[00:01.00]x").unwrap();
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));

    let lines = r#"[{"startMs":1000,"text":"x"}]"#;
    assert_eq!(stored(&store), [("1".into(), "synced".into(), None, Some(lines.into()))]);
}

/// A file rescanned with its lyrics as they were leaves the library's lyrics version, so the lyric
/// index is not rebuilt; lyrics changed or removed move it (`0007_catalog_counts.sql`).
#[test]
fn only_lyrics_that_change_move_the_lyrics_version() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(&root, "a/1.wav", &[(ItemKey::TrackTitle, "1"), (ItemKey::UnsyncLyrics, "Hello")]);
    let store = make(&root);
    let lib = testing::library(&root);
    let read = || {
        (
            scalar::<i64>(&store, "SELECT lyrics_version FROM libraries").unwrap(),
            scalar::<String>(&store, "SELECT plain FROM track_lyrics"),
        )
    };
    testing::scan_library(&store, &lib);
    let (first, _) = read();
    assert!(first > 0, "lyrics added");
    let scan = |items: &[(ItemKey, &str)]| {
        retag(&root, "a/1.wav", items);
        assert_eq!(testing::scan_library(&store, &lib).progress.updated, 1);
        read()
    };
    let (version, plain) = scan(&[(ItemKey::TrackTitle, "One"), (ItemKey::UnsyncLyrics, "Hello")]);
    assert_eq!(
        (version, plain.as_deref()),
        (first, Some("Hello")),
        "retitled, lyrics as they were"
    );
    let (changed, plain) =
        scan(&[(ItemKey::TrackTitle, "One"), (ItemKey::UnsyncLyrics, "Goodbye")]);
    assert!(changed > first, "lyrics changed");
    assert_eq!(plain.as_deref(), Some("Goodbye"));
    let (removed, plain) = scan(&[(ItemKey::TrackTitle, "One"), (ItemKey::UnsyncLyrics, "")]);
    assert!(removed > changed, "lyrics removed");
    assert_eq!(plain, None);
}
