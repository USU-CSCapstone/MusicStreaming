use super::*;

/// A few silent MPEG-1 Layer III frames, 128 kb/s at 44.1 kHz, and no tags.
fn mp3() -> Vec<u8> {
    let mut frame = vec![0xFF, 0xFB, 0x90, 0x00];
    frame.resize(417, 0);
    frame.repeat(8)
}

/// A FLAC stream that is only its STREAMINFO block: 44.1 kHz, stereo, 16-bit, no samples.
fn flac() -> Vec<u8> {
    let mut file = b"fLaC".to_vec();
    // The last metadata block, STREAMINFO, 34 bytes long.
    file.extend([0x80, 0, 0, 34]);
    // Block sizes 4096, frame sizes unknown.
    file.extend([0x10, 0x00, 0x10, 0x00, 0, 0, 0, 0, 0, 0]);
    // 44100 Hz in 20 bits, then 2 channels less one in 3, then 16 bits less one in 5, then a
    // 36-bit sample count of 0.
    file.extend([0x0A, 0xC4, 0x42, 0xF0, 0, 0, 0, 0]);
    file.extend([0; 16]);
    file
}

fn karma_police() -> Tags {
    Tags {
        title: "Karma Police".into(),
        retitle: false,
        artists: vec!["Radiohead".into()],
        album_artists: vec!["Radiohead".into()],
        album: Some("OK Computer".into()),
        track: Some(6),
        disc: Some(1),
        date: Some("1997-05-21".into()),
    }
}

/// The main tag of `file`, as a reader would see it.
fn read(file: &[u8]) -> Tag {
    let tagged = Probe::new(Cursor::new(file)).guess_file_type().unwrap().read().unwrap();
    tagged.primary_tag().cloned().unwrap()
}

fn text(tag: &Tag, key: ItemKey) -> Option<&str> {
    tag.get_string(key)
}

#[test]
fn an_untagged_mp3_gets_every_tag() {
    let filled = fill(mp3(), &karma_police(), None).unwrap().unwrap();
    let all = ["title", "artist", "album artist", "album", "track number", "disc number", "date"];
    assert_eq!(filled.written, all);
    let tag = read(&filled.bytes);
    assert_eq!(text(&tag, ItemKey::TrackTitle), Some("Karma Police"));
    assert_eq!(text(&tag, ItemKey::TrackArtist), Some("Radiohead"));
    assert_eq!(text(&tag, ItemKey::AlbumArtist), Some("Radiohead"));
    assert_eq!(text(&tag, ItemKey::AlbumTitle), Some("OK Computer"));
    assert_eq!(tag.track(), Some(6));
    assert_eq!(tag.disk(), Some(1));
    assert!(text(&tag, ItemKey::RecordingDate).is_some_and(|d| d.starts_with("1997-05-21")));
    assert!(filled.bytes.ends_with(&mp3()), "the audio is untouched");
}

#[test]
fn a_flac_gets_them_too() {
    let filled = fill(flac(), &karma_police(), None).unwrap().unwrap();
    let tag = read(&filled.bytes);
    assert_eq!(text(&tag, ItemKey::TrackArtist), Some("Radiohead"));
    assert_eq!(text(&tag, ItemKey::AlbumTitle), Some("OK Computer"));
}

#[test]
fn what_the_file_has_is_kept() {
    let mut first = Tags { album: None, track: None, disc: None, date: None, ..karma_police() };
    first.title = "Karma Police (Remastered)".into();
    let file = fill(mp3(), &first, None).unwrap().unwrap().bytes;
    let filled = fill(file, &karma_police(), None).unwrap().unwrap();
    assert_eq!(filled.written, ["album", "track number", "disc number", "date"]);
    let tag = read(&filled.bytes);
    assert_eq!(text(&tag, ItemKey::TrackTitle), Some("Karma Police (Remastered)"));
}

#[test]
fn a_title_that_named_the_artist_is_replaced() {
    let named = Tags { title: "Radiohead - Karma Police".into(), ..karma_police() };
    let file = fill(mp3(), &named, None).unwrap().unwrap().bytes;
    let filled = fill(file, &Tags { retitle: true, ..karma_police() }, None).unwrap();
    // Everything else was written the first time.
    assert_eq!(filled.as_ref().map(|f| f.written.clone()), Some(vec!["title"]));
    let tag = read(&filled.unwrap().bytes);
    assert_eq!(text(&tag, ItemKey::TrackTitle), Some("Karma Police"));
}

#[test]
fn nothing_missing_is_nothing_to_write() {
    let file = fill(mp3(), &karma_police(), None).unwrap().unwrap().bytes;
    assert!(fill(file.clone(), &karma_police(), None).unwrap().is_none());
    let retitle = Tags { retitle: true, ..karma_police() };
    assert!(fill(file, &retitle, None).unwrap().is_none(), "the same title is not written again");
}

#[test]
fn several_artists_are_separated_by_semicolons() {
    let both = vec!["Daft Punk".to_string(), "Pharrell Williams".to_string()];
    let tags = Tags { artists: both.clone(), album_artists: both, ..karma_police() };
    let tag = read(&fill(mp3(), &tags, None).unwrap().unwrap().bytes);
    assert_eq!(text(&tag, ItemKey::TrackArtist), Some("Daft Punk; Pharrell Williams"));
}

#[test]
fn a_file_that_is_not_audio_is_an_error() {
    let error = fill(b"not audio at all".to_vec(), &karma_police(), None).unwrap_err();
    assert!(error.starts_with("its tags could not be read"), "{error}");
}

/// The start of a JPEG, which is all the plugin checks of one.
const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0];

#[test]
fn a_cover_is_embedded_as_the_front_cover() {
    let filled = fill(mp3(), &karma_police(), Some(JPEG.to_vec())).unwrap().unwrap();
    assert_eq!(filled.written.last(), Some(&"cover"));
    assert!(has_cover(&filled.bytes).unwrap());
    let tag = read(&filled.bytes);
    let picture = &tag.pictures()[0];
    assert_eq!(picture.pic_type(), PictureType::CoverFront);
    assert_eq!(picture.mime_type(), Some(&MimeType::Jpeg));
    assert_eq!(picture.data(), JPEG);
}

#[test]
fn a_file_with_a_picture_keeps_it() {
    let file = fill(mp3(), &karma_police(), Some(JPEG.to_vec())).unwrap().unwrap().bytes;
    let other = [JPEG, b"other"].concat();
    assert!(fill(file.clone(), &karma_police(), Some(other)).unwrap().is_none());
    assert_eq!(read(&file).pictures()[0].data(), JPEG);
}

#[test]
fn what_is_not_an_image_is_not_embedded() {
    assert!(!has_cover(&mp3()).unwrap());
    let html = b"<html>not found</html>".to_vec();
    let filled = fill(mp3(), &karma_police(), Some(html)).unwrap().unwrap();
    assert!(!filled.written.contains(&"cover"));
    assert!(!has_cover(&filled.bytes).unwrap());
}
