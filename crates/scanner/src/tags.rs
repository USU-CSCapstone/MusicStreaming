//! Stage 3: read tags and technical properties into the canonical types
//! (`design/scanning.md` §3, §4).
//!
//! lofty is the only tag reader. It exposes most fields through a
//! scheme-independent key; the mapper below handles what survives that
//! abstraction. Nothing outside this module knows what an ID3 frame is.

use std::fs::File;
use std::io;
use std::panic;
use std::path::Path;

use jewelcase_core::Format;
use lofty::config::ParseOptions;
use lofty::error::{FileParseError, UnknownFormatError, UnsupportedTagError};
use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
use lofty::mp4::{Mp4Codec, Mp4File};
use lofty::mpeg::{Layer, MpegFile};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag, TagType};

use crate::audio::AudioProperties;
use crate::{Lyrics, PartialDate, TagSet, multi_value};

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
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

    let (tagged, format): (TaggedFile, Format) = if sniff_mp4(&mut file)? {
        let mp4 = Mp4File::read_from(&mut file, options).map_err(map_parse_error)?;
        let format = match mp4.properties().codec() {
            Some(Mp4Codec::AAC) => Format::Aac,
            Some(Mp4Codec::ALAC) => Format::Alac,
            other => return Err(ReadError::Unsupported(format!("MP4 codec {other:?}"))),
        };
        (mp4.into(), format)
    } else {
        let probe = Probe::new(&mut file).options(options).guess_file_type()?;
        if probe.file_type() == Some(FileType::Mpeg) {
            // Layers I and II share MP3's frame sync, so only the frame header tells them apart.
            let mpeg = MpegFile::read_from(probe.into_inner(), options).map_err(map_parse_error)?;
            match mpeg.properties().layer() {
                Layer::Layer3 => (mpeg.into(), Format::Mp3),
                other => return Err(ReadError::Unsupported(format!("MPEG {other:?}"))),
            }
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
            (tagged, format)
        }
    };

    let props = tagged.properties();
    let properties = AudioProperties {
        format,
        duration_ms: props.duration().as_millis() as u64,
        sample_rate: props.sample_rate(),
        bit_depth: props.bit_depth(),
        channels: props.channels(),
        bitrate_kbps: props.audio_bitrate().or(props.overall_bitrate()),
    };

    let tags = map_tags(&tagged);
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
/// first, then the rest in lofty's order. ID3v1 and RIFF INFO are last
/// resorts because they truncate.
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

/// Parse a number that may be packed as `n/total`.
fn number_pair(raw: &str) -> (Option<u32>, Option<u32>) {
    let mut it = raw.split('/');
    let n = it.next().and_then(|s| s.trim().parse().ok()).filter(|n| *n > 0);
    let total = it.next().and_then(|s| s.trim().parse().ok()).filter(|n| *n > 0);
    (n, total)
}

fn flag(raw: &str) -> bool {
    matches!(raw.trim(), "1" | "true" | "True" | "TRUE" | "yes")
}

fn map_tags(file: &TaggedFile) -> TagSet {
    let tags = ordered_tags(file);
    let mut out = TagSet::default();
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

    // Track and disc: separate keys where the scheme has them, `n/total`
    // packed where it does not. Whichever is present wins.
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

    // Date at tag precision: recording date, release date, bare year, in
    // that order. ID3v2.3's TYER+TDAT and v2.4's TDRC both arrive as
    // RecordingDate through lofty.
    out.release_date =
        [ItemKey::RecordingDate, ItemKey::ReleaseDate, ItemKey::Year, ItemKey::OriginalReleaseDate]
            .iter()
            .find_map(|k| first_string(&tags, k).and_then(|s| PartialDate::parse(&s)));

    out.genres = strings(&tags, &ItemKey::Genre);
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
    fn packed_numbers() {
        assert_eq!(number_pair("3/12"), (Some(3), Some(12)));
        assert_eq!(number_pair("3"), (Some(3), None));
        assert_eq!(number_pair("0/0"), (None, None));
        assert_eq!(number_pair("x"), (None, None));
    }
}
