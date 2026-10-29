//! Stage 3: read tags and technical properties into the canonical types
//! (`design/scanning.md` §3, §4).
//!
//! lofty is the only tag reader. It exposes most fields through a
//! scheme-independent key; the mapper below handles what survives that
//! abstraction. Nothing outside this module knows what an ID3 frame is.

use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::panic;
use std::path::Path;

use jewelcase_core::Format;
use lofty::config::ParseOptions;
use lofty::error::{FileParseError, UnknownFormatError, UnsupportedTagError};
use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
use lofty::id3::v2::{Frame, Id3v2Tag};
use lofty::iff::aiff::AiffFile;
use lofty::iff::wav::WavFile;
use lofty::mp4::{Mp4Codec, Mp4File};
use lofty::mpeg::{Layer, MpegFile};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag, TagType};

use crate::audio::{AudioProperties, duration_ms};
use crate::{Lyrics, PartialDate, TagSet, multi_value};

/// Why stage 3 could not read a file.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// The file could not be opened or read.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The content is a known container with a codec Jewelcase does not play.
    #[error("unsupported content: {0}")]
    Unsupported(String),
    /// The content is not any format lofty knows, whatever the extension
    /// claimed: a text file named `.flac`, a truncated download.
    #[error("unrecognized content: {0}")]
    Unrecognized(String),
    /// The container or its tags could not be parsed.
    #[error("malformed: {0}")]
    Malformed(String),
}

/// What stage 3 produces for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadResult {
    pub tags: TagSet,
    pub properties: AudioProperties,
    /// The front cover embedded in the file, described for the `images` table.
    pub embedded_art: Option<crate::image::ImageInfo>,
}

/// Read one file. A panic while parsing it, in lofty or the mapping, is
/// [`ReadError::Malformed`] with a fixed reason, never the panic's message,
/// which may name a host path.
pub fn read(path: &Path) -> Result<ReadResult, ReadError> {
    // The closure borrows only `path`, and everything the parse builds is dropped as it unwinds,
    // so nothing a panic interrupted is used again.
    panic::catch_unwind(|| parse(path))
        .unwrap_or_else(|_| Err(ReadError::Malformed("the tag reader failed unexpectedly".into())))
}

/// [`read`] without the panic guard.
fn parse(path: &Path) -> Result<ReadResult, ReadError> {
    let mut file = File::open(path)?;
    let options = ParseOptions::new();

    // The formats that carry ID3v2 are read by their own readers, whose ID3v2 tag still holds the
    // `TXXX` frames lofty's generic tag drops ([`user_texts`]).
    let (tagged, format, user_texts): (TaggedFile, Format, UserTexts) = if sniff_mp4(&mut file)? {
        let mp4 = Mp4File::read_from(&mut file, options).map_err(map_parse_error)?;
        let format = match mp4.properties().codec() {
            Some(Mp4Codec::AAC) => Format::Aac,
            Some(Mp4Codec::ALAC) => Format::Alac,
            other => return Err(ReadError::Unsupported(format!("MP4 codec {other:?}"))),
        };
        (mp4.into(), format, UserTexts::new())
    } else {
        let probe = Probe::new(&mut file).options(options).guess_file_type()?;
        if probe.file_type() == Some(FileType::Mpeg) {
            // Layers I and II share MP3's frame sync, so only the frame header tells them apart.
            let mpeg = MpegFile::read_from(probe.into_inner(), options).map_err(map_parse_error)?;
            let user_texts = user_texts(mpeg.id3v2());
            match mpeg.properties().layer() {
                Layer::Layer3 => (mpeg.into(), Format::Mp3, user_texts),
                other => return Err(ReadError::Unsupported(format!("MPEG {other:?}"))),
            }
        } else if probe.file_type() == Some(FileType::Wav) {
            let wav = WavFile::read_from(probe.into_inner(), options).map_err(map_parse_error)?;
            let user_texts = user_texts(wav.id3v2());
            (wav.into(), Format::Wav, user_texts)
        } else if probe.file_type() == Some(FileType::Aiff) {
            let aiff = AiffFile::read_from(probe.into_inner(), options).map_err(map_parse_error)?;
            let user_texts = user_texts(aiff.id3v2());
            (aiff.into(), Format::Aiff, user_texts)
        } else {
            let tagged = probe.read().map_err(map_parse_error)?;
            let format = match tagged.file_type() {
                FileType::Flac => Format::Flac,
                FileType::Aac => Format::Aac,
                FileType::Vorbis => Format::Vorbis,
                FileType::Opus => Format::Opus,
                FileType::Wav => Format::Wav,
                FileType::Aiff => Format::Aiff,
                // MP4 without an `ftyp` sniff hit; lofty found it anyway.
                FileType::Mp4 => Format::Aac,
                other => return Err(ReadError::Unsupported(format!("{other:?}"))),
            };
            (tagged, format, UserTexts::new())
        }
    };

    let props = tagged.properties();
    let properties = AudioProperties {
        format,
        duration_ms: duration_ms(props.duration()),
        sample_rate: props.sample_rate(),
        bit_depth: props.bit_depth(),
        channels: props.channels(),
        bitrate_kbps: props.audio_bitrate().or(props.overall_bitrate()),
    };

    let tags = map_tags(&tagged, &user_texts);
    let embedded_art = front_cover(&tagged).and_then(crate::image::describe);
    Ok(ReadResult { tags, properties, embedded_art })
}

/// `ftyp` at offset 4 marks an ISO base media file. lofty's guess handles it
/// too, but the MP4 path needs the typed reader to learn the codec.
fn sniff_mp4(file: &mut File) -> io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut head = [0u8; 12];
    let n = file.read(&mut head)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(n >= 8 && &head[4..8] == b"ftyp")
}

/// The [`ReadError`] a lofty parse error is: unrecognized or unsupported content where its causes
/// or message say so, a permission error as I/O, and anything else malformed.
fn map_parse_error(e: FileParseError) -> ReadError {
    use std::error::Error as _;
    let mut source = e.source();
    while let Some(s) = source {
        if s.is::<UnknownFormatError>() {
            return ReadError::Unrecognized(e.to_string());
        }
        if s.is::<UnsupportedTagError>() {
            return ReadError::Unsupported(e.to_string());
        }
        if let Some(io) = s.downcast_ref::<io::Error>()
            && io.kind() == io::ErrorKind::PermissionDenied
        {
            return ReadError::Io(io::Error::new(io::ErrorKind::PermissionDenied, io.to_string()));
        }
        source = s.source();
    }
    let text = e.to_string().to_ascii_lowercase();
    if text.contains("unknown format") {
        return ReadError::Unrecognized(e.to_string());
    }
    if text.contains("unsupported") {
        return ReadError::Unsupported(e.to_string());
    }
    ReadError::Malformed(e.to_string())
}

/// The front cover's bytes: a picture typed as such, else the first picture.
fn front_cover(file: &TaggedFile) -> Option<&[u8]> {
    let tags = file.tags();
    tags.iter()
        .find_map(|t| t.get_picture_type(PictureType::CoverFront))
        .or_else(|| tags.iter().find_map(|t| t.pictures().first()))
        .map(|p| p.data())
}

// ---------------------------------------------------------------------------
// The mapper

/// Tags in the order fields are taken from them: the format's primary tag
/// first, then ID3v2, Vorbis comments and MP4, then APE, then the rest, and
/// ID3v1 last, since it truncates. Tags of one rank keep lofty's order.
fn ordered_tags(file: &TaggedFile) -> Vec<&Tag> {
    let mut tags: Vec<&Tag> = file.tags().iter().collect();
    let rank = |t: &Tag| match t.tag_type() {
        TagType::Id3v2 | TagType::VorbisComments | TagType::Mp4Ilst => 0,
        TagType::Ape => 1,
        TagType::AiffText | TagType::RiffInfo => 2,
        TagType::Id3v1 => 3,
        _ => 2,
    };
    tags.sort_by_key(|t| rank(t));
    if let Some(primary) = file.primary_tag()
        && let Some(pos) = tags.iter().position(|t| std::ptr::eq(*t, primary))
    {
        let p = tags.remove(pos);
        tags.insert(0, p);
    }
    tags
}

/// First non-empty string for `key` across the tags, in priority order.
fn first_string(tags: &[&Tag], key: &ItemKey) -> Option<String> {
    tags.iter().find_map(|t| {
        t.get_string(*key).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
    })
}

/// All values for `key` from the first tag that has any. Native repeats and
/// semicolons come out the same (`requirements/tags.md` §3).
fn strings(tags: &[&Tag], key: &ItemKey) -> Vec<String> {
    for t in tags {
        let values = multi_value::split_all(t.get_strings(*key));
        if !values.is_empty() {
            return values;
        }
    }
    Vec::new()
}

/// The record labels: the label tag's values, else the publisher tag's, in tagged order, each
/// once by its name key with its first spelling ([`strings`]). ID3v2 has one frame for both,
/// `TPUB`, which lofty reads as the publisher; Vorbis comments have `LABEL` (or `ORGANIZATION`)
/// and `PUBLISHER`, and MP4 has `----:com.apple.iTunes:LABEL`.
fn labels(tags: &[&Tag]) -> Vec<String> {
    let labels = strings(tags, &ItemKey::Label);
    if labels.is_empty() { strings(tags, &ItemKey::Publisher) } else { labels }
}

/// A tag type the mapper reads (`design/scanning.md` §4): its key, as `tag_types` lists it, the
/// lofty key its values come from, the description of an ID3v2 `TXXX` frame that carries them
/// where lofty maps none, and the generation of types it was added in.
struct TagTypeField {
    key: &'static str,
    item: ItemKey,
    txxx: Option<&'static str>,
    since: u32,
}

/// Every tag type the scanner reads (`requirements/tags.md` §2). Each is read the same way, by
/// [`strings`] and then [`user_texts`], so a new type is an entry here, in the generation after
/// the newest. ID3v2.3 has no mood frame, so taggers write mood there as `TXXX:MOOD`.
const TAG_TYPES: &[TagTypeField] = &[
    TagTypeField { key: "genre", item: ItemKey::Genre, txxx: None, since: 1 },
    TagTypeField { key: "mood", item: ItemKey::Mood, txxx: Some("MOOD"), since: 2 },
];

/// The values of the ID3v2 `TXXX` frames that tag types read ([`TagTypeField::txxx`]), by type
/// key. lofty's generic tag drops a `TXXX` frame whose description it does not map.
type UserTexts = BTreeMap<&'static str, Vec<String>>;

/// The values in `tag`'s `TXXX` frames for each tag type that reads one, its description matched
/// ignoring case, as taggers differ on it. Native null-separated values and semicolons come out
/// the same, as in [`strings`].
fn user_texts(tag: Option<&Id3v2Tag>) -> UserTexts {
    let mut out = UserTexts::new();
    let Some(tag) = tag else { return out };
    for tag_type in TAG_TYPES {
        let Some(description) = tag_type.txxx else { continue };
        let frames = tag.into_iter().filter_map(|frame| match frame {
            Frame::UserText(text) if text.description.eq_ignore_ascii_case(description) => {
                Some(text.content.as_ref())
            }
            _ => None,
        });
        let values = multi_value::split_all(frames.flat_map(|content| content.split('\0')));
        if !values.is_empty() {
            out.insert(tag_type.key, values);
        }
    }
    out
}

/// The keys of every tag type the scanner reads, each of which `tag_types` must list.
pub fn tag_type_keys() -> impl Iterator<Item = &'static str> {
    TAG_TYPES.iter().map(|tag_type| tag_type.key)
}

/// The generation of the tag types the scanner reads: the one the newest was added in. A track
/// read with an older one is read again, though its file is unchanged, for the types added
/// since (`design/scanning.md` §2).
pub fn tags_generation() -> u32 {
    TAG_TYPES.iter().map(|tag_type| tag_type.since).max().unwrap_or(0)
}

/// Parse a number that may be packed as `n/total`.
fn number_pair(raw: &str) -> (Option<u32>, Option<u32>) {
    let mut it = raw.split('/');
    let n = it.next().and_then(|s| s.trim().parse().ok()).filter(|n| *n > 0);
    let total = it.next().and_then(|s| s.trim().parse().ok()).filter(|n| *n > 0);
    (n, total)
}

/// Whether a flag tag's value is set.
fn flag(raw: &str) -> bool {
    matches!(raw.trim(), "1" | "true" | "True" | "TRUE" | "yes")
}

/// The canonical [`TagSet`] of `file`'s tags, with `user_texts` for the tag types lofty's
/// generic tag carries nothing of.
fn map_tags(file: &TaggedFile, user_texts: &UserTexts) -> TagSet {
    let tags = ordered_tags(file);
    let mut out = TagSet::default();
    for tag_type in TAG_TYPES {
        let values = strings(&tags, &tag_type.item);
        let values = if values.is_empty() {
            user_texts.get(tag_type.key).cloned().unwrap_or_default()
        } else {
            values
        };
        out.set_values(tag_type.key, values);
    }
    if tags.is_empty() {
        return out;
    }

    out.title = first_string(&tags, &ItemKey::TrackTitle);
    out.artists = strings(&tags, &ItemKey::TrackArtist);
    if out.artists.is_empty() {
        out.artists = strings(&tags, &ItemKey::TrackArtists);
    }
    out.album = first_string(&tags, &ItemKey::AlbumTitle);
    out.album_artists = strings(&tags, &ItemKey::AlbumArtist);
    if out.album_artists.is_empty() {
        out.album_artists = strings(&tags, &ItemKey::AlbumArtists);
    }
    out.compilation = first_string(&tags, &ItemKey::FlagCompilation).is_some_and(|v| flag(&v));

    // Track and disc: `n/total` packed where the scheme has no separate total
    // key; a separate total, where present, wins over a packed one.
    if let Some(raw) = first_string(&tags, &ItemKey::TrackNumber) {
        let (n, total) = number_pair(&raw);
        out.track_number = n;
        out.track_total = total;
    }
    if let Some(total) =
        first_string(&tags, &ItemKey::TrackTotal).and_then(|s| s.trim().parse().ok())
    {
        out.track_total = Some(total);
    }
    if let Some(raw) = first_string(&tags, &ItemKey::DiscNumber) {
        let (n, total) = number_pair(&raw);
        out.disc_number = n;
        out.disc_total = total;
    }
    if let Some(total) =
        first_string(&tags, &ItemKey::DiscTotal).and_then(|s| s.trim().parse().ok())
    {
        out.disc_total = Some(total);
    }

    // Date at tag precision: recording date, release date, bare year, then
    // original release date. ID3v2.3's TYER+TDAT and v2.4's TDRC both arrive
    // as RecordingDate through lofty.
    out.release_date =
        [ItemKey::RecordingDate, ItemKey::ReleaseDate, ItemKey::Year, ItemKey::OriginalReleaseDate]
            .iter()
            .find_map(|k| first_string(&tags, k).and_then(|s| PartialDate::parse(&s)));

    out.release_type =
        first_string(&tags, &ItemKey::MusicBrainzReleaseType).map(|s| s.to_ascii_lowercase());
    out.labels = labels(&tags);
    out.explicit = first_string(&tags, &ItemKey::ParentalAdvisory).and_then(|v| match v.trim() {
        "1" | "4" | "explicit" | "Explicit" => Some(true),
        "2" | "clean" | "Clean" => Some(false),
        _ => None,
    });
    out.isrc = first_string(&tags, &ItemKey::Isrc);
    out.musicbrainz_recording_id = first_string(&tags, &ItemKey::MusicBrainzRecordingId);
    out.musicbrainz_release_id = first_string(&tags, &ItemKey::MusicBrainzReleaseId);

    out.lyrics = first_string(&tags, &ItemKey::Lyrics)
        .or_else(|| first_string(&tags, &ItemKey::UnsyncLyrics))
        .and_then(|text| Lyrics::parse(&text));

    out.has_embedded_art = tags.iter().any(|t| !t.pictures().is_empty());

    out.title_sort = first_string(&tags, &ItemKey::TrackTitleSortOrder);
    out.artist_sort = first_string(&tags, &ItemKey::TrackArtistSortOrder);
    out.album_sort = first_string(&tags, &ItemKey::AlbumTitleSortOrder);
    out.album_artist_sort = first_string(&tags, &ItemKey::AlbumArtistSortOrder);
    out
}

#[cfg(test)]
mod tests {
    use lofty::tag::{ItemValue, TagItem};

    use super::*;

    /// A Vorbis comment holding `items`.
    fn tagged(items: &[(ItemKey, &str)]) -> Tag {
        let mut tag = Tag::new(TagType::VorbisComments);
        for &(key, value) in items {
            assert!(tag.push(TagItem::new(key, ItemValue::Text(value.into()))));
        }
        tag
    }

    #[test]
    fn labels_come_from_the_label_tag_else_the_publisher_tag_once_each() {
        let both = tagged(&[
            (ItemKey::Publisher, "Warner"),
            (ItemKey::Label, "Sub Pop"),
            (ItemKey::Label, "SUB POP;Domino"),
        ]);
        assert_eq!(labels(&[&both]), ["Sub Pop", "Domino"]);
        let publisher = tagged(&[(ItemKey::Publisher, "Warner;WARNER")]);
        assert_eq!(labels(&[&publisher]), ["Warner"]);
        assert!(labels(&[&tagged(&[])]).is_empty());
    }

    #[test]
    fn a_user_text_frame_carries_a_type_lofty_does_not_map() {
        use lofty::TextEncoding;
        use lofty::id3::v2::ExtendedTextFrame;
        let mut tag = Id3v2Tag::new();
        for (description, content) in [("mood", "Calm\0Dark;calm"), ("OTHER", "Loud")] {
            tag.insert(Frame::UserText(ExtendedTextFrame::new(
                TextEncoding::UTF8,
                description,
                content,
            )));
        }
        let mood = vec!["Calm".to_owned(), "Dark".to_owned()];
        assert_eq!(user_texts(Some(&tag)), UserTexts::from([("mood", mood)]));
        assert!(user_texts(None).is_empty());
    }

    #[test]
    fn packed_numbers() {
        assert_eq!(number_pair("3/12"), (Some(3), Some(12)));
        assert_eq!(number_pair("3"), (Some(3), None));
        assert_eq!(number_pair("0/0"), (None, None));
        assert_eq!(number_pair("x"), (None, None));
    }
}
