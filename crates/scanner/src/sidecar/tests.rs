use std::path::Path;

use super::*;
use crate::SyncedLine;
use crate::clock::system_time_ms;

/// A 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// Writes `bytes` to the file at `p`, creating the folders above it.
fn touch(p: &Path, bytes: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

/// The regular files in `dir`, as discovery lists them.
fn listing(dir: &Path) -> BTreeMap<OsString, FileStat> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_file())
        .map(|entry| {
            let meta = entry.metadata().unwrap();
            let mtime_ms = system_time_ms(meta.modified().unwrap());
            (entry.file_name(), FileStat { size: meta.len(), mtime_ms })
        })
        .collect()
}

/// What a fresh scan of `root` stamps the track at `audio` with, with every directory from the
/// root down to it listed as a walk lists them.
fn stamped(root: &Path, audio: &Path) -> Stamped {
    let mut r = SidecarResolver::new(root);
    let dir = audio.parent().unwrap();
    for ancestor in dir.ancestors().take_while(|a| a.starts_with(root)) {
        r.offer_listing(ancestor, &listing(ancestor));
    }
    r.stamp(dir, audio, &listing(dir))
}

/// The stamp [`stamped`] gives.
fn stamp(root: &Path, audio: &Path) -> SidecarStamp {
    stamped(root, audio).stamp
}

/// The lyrics a scan of the directory holding `audio` reads for it.
fn lyrics(audio: &Path) -> Option<Lyrics> {
    stamped(audio.parent().unwrap(), audio).read_lyrics()
}

#[test]
fn nearest_wins_and_walk_stops_at_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    touch(&root.join("cover.png"), PNG);
    touch(&root.join("Artist/artist.jpg"), PNG);
    // Saved with a byte order mark, which is not part of the text.
    touch(&root.join("Artist/artist.txt"), b"\xEF\xBB\xBFA band.\n");
    touch(&root.join("Artist/Album/cover.jpg"), PNG);
    touch(&root.join("Artist/Other/01.flac"), b"x");
    // Above the root: must never be seen.
    touch(&tmp.path().join("artist.png"), PNG);

    let mut r = SidecarResolver::new(&root);
    let album = r.resolve(&root.join("Artist/Album"));
    assert_eq!(album.cover.unwrap().path, root.join("Artist/Album/cover.jpg"));
    assert_eq!(album.artist_image.unwrap().path, root.join("Artist/artist.jpg"));
    assert_eq!(&*album.biography.unwrap().text, "A band.");

    let other = r.resolve(&root.join("Artist/Other"));
    assert_eq!(other.cover.unwrap().path, root.join("cover.png"));

    let top = r.resolve(&root);
    assert_eq!(top.artist_image, None);
}

/// A `cover.*` that is there but not an image is no art, and the walk stops at it rather than
/// take the cover above, which the stamp of the tracks below it does not hash.
#[test]
fn a_cover_that_is_not_an_image_is_no_art() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    touch(&root.join("cover.png"), PNG);
    touch(&root.join("A/cover.jpg"), b"not an image");
    let mut r = SidecarResolver::new(&root);
    assert_eq!(r.resolve(&root.join("A")).cover, None);
}

#[test]
fn lyrics_match_stem_only_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    touch(&dir.join("01 - Song.flac"), b"x");
    touch(&dir.join("01 - Song.lrc"), b"[00:01.00]hi");
    touch(&dir.join("02 - Other.flac"), b"x");
    touch(&dir.join("02 - Other.TXT"), b"plain words");
    assert_eq!(
        lyrics(&dir.join("01 - Song.flac")),
        Some(Lyrics::Synced(vec![SyncedLine { start_ms: 1_000, text: "hi".into() }]))
    );
    assert_eq!(lyrics(&dir.join("02 - Other.flac")), Some(Lyrics::Plain("plain words".into())));
    assert_eq!(lyrics(&dir.join("03.flac")), None);
}

#[test]
fn symlinked_sidecars_are_not_read() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    touch(&root.join("A/01.flac"), b"x");
    touch(&root.join("bio.txt"), b"A band.");
    touch(&root.join("words.lrc"), b"[00:01.00]words");
    touch(&root.join("cover.png"), PNG);
    symlink(root.join("bio.txt"), root.join("A/artist.txt")).unwrap();
    symlink(root.join("words.lrc"), root.join("A/01.lrc")).unwrap();
    symlink(root.join("cover.png"), root.join("A/cover.png")).unwrap();

    let mut r = SidecarResolver::new(&root);
    let found = r.resolve(&root.join("A"));
    assert_eq!(found.biography, None);
    assert_eq!(found.cover.unwrap().path, root.join("cover.png"), "the real one above");
    assert_eq!(lyrics(&root.join("A/01.flac")), None);
}

/// A track's stamp changes when a sidecar that applies to it is added, changed or removed, its
/// lyrics file included, and only then. An empty lyrics file is no lyrics, so it stamps as none.
#[test]
fn the_stamp_follows_each_sidecar_that_applies() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let audio = root.join("A/X/01.flac");
    touch(&audio, b"x");
    let mut seen = vec![stamp(&root, &audio)];
    assert_eq!(stamp(&root, &audio), seen[0], "nothing changed");

    let lrc = root.join("A/X/01.lrc");
    touch(&lrc, b"");
    assert_eq!(stamp(&root, &audio), seen[0], "an empty lyrics file is none");
    touch(&root.join("A/X/02.lrc"), b"[00:01.00]another track's");
    assert_eq!(stamp(&root, &audio), seen[0], "another track's lyrics");

    let changes: [(&Path, &[u8]); 5] = [
        (&root.join("A/X/cover.png"), PNG),
        (&root.join("A/artist.txt"), b"A band."),
        (&root.join("A/artist.png"), PNG),
        (&lrc, b"[00:01.00]words"),
        (&root.join("A/artist.txt"), b"A longer biography."),
    ];
    for (path, bytes) in changes {
        touch(path, bytes);
        let now = stamp(&root, &audio);
        assert!(!seen.contains(&now), "{} changed", path.display());
        seen.push(now);
    }
    fs::remove_file(root.join("A/X/cover.png")).unwrap();
    assert!(!seen.contains(&stamp(&root, &audio)), "the cover removed");
}

/// A lyrics file rewritten at the same size is noticed by its modified time.
#[test]
fn a_same_size_rewrite_changes_the_stamp() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let audio = root.join("A/01.flac");
    touch(&audio, b"x");
    let lrc = root.join("A/01.lrc");
    touch(&lrc, b"[00:01.00]aaaa");
    let before = stamp(&root, &audio);
    touch(&lrc, b"[00:01.00]bbbb");
    let later = fs::metadata(&lrc).unwrap().modified().unwrap() + std::time::Duration::from_secs(5);
    fs::File::options().write(true).open(&lrc).unwrap().set_modified(later).unwrap();
    assert_ne!(stamp(&root, &audio), before);
}

/// A folder above a folder-scoped scan, which the walk does not list, is located as discovery
/// would list it: its regular files only, with the same sizes and modified times, so its tracks
/// stamp and resolve as a whole scan's do. Here a symlinked `cover.jpg` sits before the real
/// `cover.png`, and an empty `cover.jpg` before the root's `cover.png`, each giving way to it.
#[test]
fn an_unlisted_ancestor_locates_as_a_listed_one() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let audio = root.join("A/X/01.flac");
    touch(&audio, b"x");
    touch(&root.join("cover.jpg"), b"");
    touch(&root.join("cover.png"), PNG);
    touch(&root.join("A/cover.png"), &[PNG, b"A"].concat());
    touch(&root.join("A/artist.txt"), b"A band.");
    touch(&tmp.path().join("elsewhere.png"), PNG);
    symlink(tmp.path().join("elsewhere.png"), root.join("A/cover.jpg")).unwrap();

    let dir = audio.parent().unwrap();
    let mut scoped = SidecarResolver::new(&root);
    scoped.offer_listing(dir, &listing(dir));
    assert_eq!(scoped.stamp(dir, &audio, &listing(dir)).stamp, stamp(&root, &audio));
    assert_eq!(scoped.resolve(dir).cover.unwrap().path, root.join("A/cover.png"));

    fs::remove_file(root.join("A/cover.png")).unwrap();
    let mut scoped = SidecarResolver::new(&root);
    scoped.offer_listing(dir, &listing(dir));
    assert_eq!(scoped.stamp(dir, &audio, &listing(dir)).stamp, stamp(&root, &audio));
    assert_eq!(
        scoped.resolve(dir).cover.unwrap().path,
        root.join("cover.png"),
        "past the empty one"
    );
}

/// Of two biographies whose names differ only in case, the first in byte order that is not
/// empty applies, whether the folder was listed by the walk or by the resolver itself, whose
/// listing comes in no particular order.
#[test]
fn of_biographies_differing_in_case_the_first_usable_applies() {
    for (upper, lower, expected) in
        [(&b""[..], &b"Lower."[..], "Lower."), (b"Upper.", b"", "Upper.")]
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        let audio = root.join("A/X/01.flac");
        touch(&audio, b"x");
        touch(&root.join("A/ARTIST.TXT"), upper);
        touch(&root.join("A/artist.txt"), lower);
        let dir = audio.parent().unwrap();
        let mut scoped = SidecarResolver::new(&root);
        scoped.offer_listing(dir, &listing(dir));
        assert_eq!(scoped.stamp(dir, &audio, &listing(dir)).stamp, stamp(&root, &audio));
        assert_eq!(
            scoped.resolve(dir).biography.map(|b| b.text.to_string()).as_deref(),
            Some(expected)
        );
        let mut listed = SidecarResolver::new(&root);
        listed.offer_listing(&root.join("A"), &listing(&root.join("A")));
        assert_eq!(
            listed.resolve(dir).biography.map(|b| b.text.to_string()).as_deref(),
            Some(expected)
        );
    }
}

/// Of covers with different extensions, the first in [`IMAGE_EXTENSIONS`] order applies.
#[test]
fn a_png_cover_comes_before_a_gif() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    touch(&root.join("A/cover.gif"), &[PNG, b"gif"].concat());
    touch(&root.join("A/cover.png"), PNG);
    let mut r = SidecarResolver::new(&root);
    r.offer_listing(&root.join("A"), &listing(&root.join("A")));
    assert_eq!(r.resolve(&root.join("A")).cover.unwrap().path, root.join("A/cover.png"));
}

/// The lyrics of a track in one folder are found after another folder's tracks were stamped, by a
/// name differing in case too.
#[test]
fn lyrics_are_found_in_each_folder_in_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let (a, b) = (root.join("A/01.flac"), root.join("B/01.flac"));
    touch(&a, b"x");
    touch(&b, b"x");
    touch(&root.join("B/01.LRC"), b"[00:01.00]bee");
    let mut r = SidecarResolver::new(&root);
    let (dir_a, dir_b) = (root.join("A"), root.join("B"));
    assert_eq!(r.stamp(&dir_a, &a, &listing(&dir_a)).read_lyrics(), None);
    let lines = vec![SyncedLine { start_ms: 1_000, text: "bee".into() }];
    assert_eq!(r.stamp(&dir_b, &b, &listing(&dir_b)).read_lyrics(), Some(Lyrics::Synced(lines)));
}

/// A sidecar a nearer one overrides is not in the stamp, so changing it reads no track that does
/// not resolve it.
#[test]
fn an_overridden_sidecar_is_not_in_the_stamp() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let audio = root.join("A/X/01.flac");
    touch(&audio, b"x");
    touch(&root.join("cover.png"), PNG);
    touch(&root.join("A/X/cover.png"), PNG);
    let before = stamp(&root, &audio);
    touch(&root.join("cover.png"), &[PNG, b"more"].concat());
    assert_eq!(stamp(&root, &audio), before);
}

#[test]
fn sidecar_names() {
    assert!(is_sidecar_name(OsStr::new("cover.jpg")));
    assert!(is_sidecar_name(OsStr::new("Cover.PNG")));
    assert!(is_sidecar_name(OsStr::new("artist.txt")));
    assert!(!is_sidecar_name(OsStr::new("cover.txt")));
    assert!(!is_sidecar_name(OsStr::new("front.jpg")));
}
