//! The golden corpus (`design/scanning.md` §4, §15): one logical album in
//! every supported format must read back as byte-identical `TagSet`s, and
//! content, not the extension, decides the format (§3).
//!
//! Needs ffmpeg on PATH to produce the encoded files, and fails without it.

use std::collections::BTreeMap;
use std::path::Path;

use jewelcase_core::Format;
use jewelcase_scanner::testing::{PNG, encode, write_sine_wav, write_small_wav};
use jewelcase_scanner::{Lyrics, PartialDate, ReadError, SyncedLine, TagSet, read_tags};
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{ItemKey, ItemValue, Tag, TagItem, TagType};

/// The embedded lyrics every file carries, which read back as [`expected`]'s timed lines.
const LYRICS: &str = "[00:12.00]Load up on guns\n[00:15.00]Bring your friends";

/// The tags every file in the corpus is written with, and must read back as.
fn expected() -> TagSet {
    TagSet {
        title: Some("Smells Like Teen Spirit".into()),
        artists: vec!["Nirvana".into(), "Dave Grohl".into()],
        album: Some("Nevermind".into()),
        album_artists: vec!["Nirvana".into()],
        compilation: true,
        track_number: Some(1),
        track_total: Some(12),
        disc_number: Some(1),
        disc_total: Some(2),
        release_date: Some(PartialDate { year: 1991, month: Some(9), day: Some(24) }),
        tag_values: BTreeMap::from([
            ("genre".into(), vec!["Grunge".into(), "Rock".into()]),
            ("mood".into(), vec!["Angry".into(), "Restless".into()]),
        ]),
        release_type: Some("album".into()),
        labels: vec!["DGC".into()],
        explicit: None,
        isrc: Some("USGF19942501".into()),
        musicbrainz_recording_id: Some("2bd6a4a6-8c8a-4d2d-9a4b-0a6a9b2a9c1d".into()),
        musicbrainz_release_id: None,
        lyrics: Some(Lyrics::Synced(vec![
            SyncedLine { start_ms: 12_000, text: "Load up on guns".into() },
            SyncedLine { start_ms: 15_000, text: "Bring your friends".into() },
        ])),
        has_embedded_art: true,
        title_sort: Some("Smells Like Teen Spirit".into()),
        artist_sort: Some("Nirvana".into()),
        album_artist_sort: Some("Nirvana, The".into()),
        album_sort: Some("Nevermind".into()),
    }
}

/// Tag the file at `path` with [`expected`]'s values in `tag_type`, lyrics as raw [`LYRICS`] text
/// and the cover as [`PNG`]. Panics when the tag can't be saved.
fn write_tags(path: &Path, tag_type: TagType) {
    let e = expected();
    let mut tag = Tag::new(tag_type);
    let text = |k: ItemKey, v: &str| TagItem::new(k, ItemValue::Text(v.to_owned()));
    tag.insert(text(ItemKey::TrackTitle, e.title.as_deref().unwrap()));
    for a in &e.artists {
        tag.push(text(ItemKey::TrackArtist, a));
    }
    tag.insert(text(ItemKey::AlbumTitle, e.album.as_deref().unwrap()));
    tag.insert(text(ItemKey::AlbumArtist, &e.album_artists[0]));
    tag.insert(text(ItemKey::FlagCompilation, "1"));
    tag.insert(text(ItemKey::TrackNumber, "1"));
    tag.insert(text(ItemKey::TrackTotal, "12"));
    tag.insert(text(ItemKey::DiscNumber, "1"));
    tag.insert(text(ItemKey::DiscTotal, "2"));
    tag.insert(text(ItemKey::RecordingDate, "1991-09-24"));
    for g in e.values_of("genre") {
        tag.push(text(ItemKey::Genre, g));
    }
    for m in e.values_of("mood") {
        tag.push(text(ItemKey::Mood, m));
    }
    tag.insert(text(ItemKey::MusicBrainzReleaseType, "album"));
    // ID3v2 writes the label as `TPUB`, which reads back as the publisher.
    tag.insert(text(ItemKey::Label, &e.labels[0]));
    tag.insert(text(ItemKey::Isrc, e.isrc.as_deref().unwrap()));
    tag.insert(text(
        ItemKey::MusicBrainzRecordingId,
        e.musicbrainz_recording_id.as_deref().unwrap(),
    ));
    // ID3v2 carries lyrics in USLT, which lofty exposes as `UnsyncLyrics`;
    // Vorbis comments and MP4 use `Lyrics`. Real taggers do the same, and
    // the mapper must make both read identically.
    let lyrics_key =
        if tag_type == TagType::Id3v2 { ItemKey::UnsyncLyrics } else { ItemKey::Lyrics };
    tag.insert(text(lyrics_key, LYRICS));
    tag.insert(text(ItemKey::TrackTitleSortOrder, e.title_sort.as_deref().unwrap()));
    tag.insert(text(ItemKey::TrackArtistSortOrder, e.artist_sort.as_deref().unwrap()));
    tag.insert(text(ItemKey::AlbumTitleSortOrder, e.album_sort.as_deref().unwrap()));
    tag.insert(text(ItemKey::AlbumArtistSortOrder, e.album_artist_sort.as_deref().unwrap()));
    tag.push_picture(
        Picture::unchecked(PNG.to_vec())
            .pic_type(PictureType::CoverFront)
            .mime_type(MimeType::Png)
            .build(),
    );
    tag.save_to_path(path, WriteOptions::default())
        .unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
}

#[test]
fn every_format_reads_the_same_tagset() {
    let tmp = tempfile::tempdir().unwrap();
    let wav = tmp.path().join("src.wav");
    write_sine_wav(&wav, 440.0, 0.3, 1.0);

    // (file, ffmpeg args, tag scheme lofty writes, expected format)
    let cases: Vec<(&str, Vec<&str>, TagType, Format)> = vec![
        ("t.flac", vec!["-c:a", "flac"], TagType::VorbisComments, Format::Flac),
        ("t.mp3", vec!["-c:a", "libmp3lame", "-b:a", "128k"], TagType::Id3v2, Format::Mp3),
        ("aac.m4a", vec!["-c:a", "aac", "-b:a", "128k"], TagType::Mp4Ilst, Format::Aac),
        ("alac.m4a", vec!["-c:a", "alac"], TagType::Mp4Ilst, Format::Alac),
        ("t.ogg", vec!["-c:a", "libvorbis"], TagType::VorbisComments, Format::Vorbis),
        ("t.opus", vec!["-c:a", "libopus"], TagType::VorbisComments, Format::Opus),
        ("t.aiff", vec!["-c:a", "pcm_s16be"], TagType::Id3v2, Format::Aiff),
        ("t.wav", vec!["-c:a", "pcm_s16le"], TagType::Id3v2, Format::Wav),
    ];

    let want = expected();
    let mut failures = Vec::new();
    for (name, args, tag_type, format) in cases {
        let path = tmp.path().join(name);
        encode(&wav, &path, &args);
        write_tags(&path, tag_type);
        let got = read_tags(&path).unwrap_or_else(|e| panic!("read {name}: {e}"));
        if got.properties.format != format {
            failures.push(format!("{name}: format {:?}, wanted {format:?}", got.properties.format));
        }
        if got.tags != want {
            failures.push(format!("{name}:\n  got  {:?}\n  want {:?}", got.tags, want));
        }
        // Properties are real, not assumed.
        assert_eq!(got.properties.channels, Some(2), "{name}");
        assert!(
            got.properties.duration_ms >= 900 && got.properties.duration_ms <= 1200,
            "{name}: {}",
            got.properties.duration_ms
        );
        assert!(got.properties.sample_rate.is_some(), "{name}");
    }
    assert!(failures.is_empty(), "golden corpus diverged:\n{}", failures.join("\n"));
}

#[test]
fn mpeg_layer_ii_named_mp3_is_unsupported() {
    let tmp = tempfile::tempdir().unwrap();
    let wav = tmp.path().join("src.wav");
    write_small_wav(&wav);
    let path = tmp.path().join("disguised.mp3");
    encode(&wav, &path, &["-c:a", "mp2", "-f", "mp2"]);

    match read_tags(&path) {
        Err(ReadError::Unsupported(detail)) => assert!(detail.contains("Layer2"), "{detail}"),
        other => panic!("Layer II read as {other:?}"),
    }
}

#[test]
fn id3v23_packed_totals_and_semicolons() {
    // Raw ID3v2 frames as a v2.3 tagger writes them: TRCK "1/12", TPOS
    // "1/2", artists semicolon-joined in one TPE1, genres in one TCON, and mood in a TXXX frame,
    // since v2.3 has no TMOO.
    use lofty::TextEncoding;
    use lofty::id3::v2::{ExtendedTextFrame, Frame, FrameId, Id3v2Tag, TextInformationFrame};
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("packed.wav");
    write_small_wav(&path);
    let mut tag = Id3v2Tag::new();
    let frame = |id: &'static str, v: &str| {
        Frame::Text(TextInformationFrame::new(
            FrameId::Valid(id.into()),
            TextEncoding::UTF8,
            v.to_owned(),
        ))
    };
    tag.insert(frame("TRCK", "1/12"));
    tag.insert(frame("TPOS", "1/2"));
    tag.insert(frame("TPE1", "Kendrick Lamar; SZA"));
    tag.insert(frame("TCON", "Hip-Hop;R&B"));
    tag.insert(frame("TDRC", "2018"));
    tag.insert(Frame::UserText(ExtendedTextFrame::new(TextEncoding::UTF8, "MOOD", "Dark;Dreamy")));
    tag.save_to_path(&path, WriteOptions::default()).unwrap();
    let got = read_tags(&path).unwrap().tags;
    assert_eq!(got.track_number, Some(1));
    assert_eq!(got.track_total, Some(12));
    assert_eq!(got.disc_number, Some(1));
    assert_eq!(got.disc_total, Some(2));
    assert_eq!(got.artists, vec!["Kendrick Lamar", "SZA"]);
    assert_eq!(got.values_of("genre"), ["Hip-Hop", "R&B"]);
    assert_eq!(got.values_of("mood"), ["Dark", "Dreamy"]);
    assert_eq!(got.release_date, Some(PartialDate { year: 2018, month: None, day: None }));
    assert!(!got.has_embedded_art);
}
