//! Stage 3: read tags and technical properties into the canonical types
//! (`design/scanning.md` §3, §4).
//!
//! lofty is the only tag reader. It exposes most fields through a
//! scheme-independent key; the mapper below handles what survives that
//! abstraction. Nothing outside this module knows what an ID3 frame is.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::panic;
use std::path::Path;

use jewelcase_core::Format;
use lofty::aac::AacFile;
use lofty::ape::ApeTag;
use lofty::config::ParseOptions;
use lofty::error::{FileParseError, UnknownFormatError, UnsupportedTagError};
use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::id3::v2::{Frame, Id3v2Tag};
use lofty::iff::aiff::AiffFile;
use lofty::iff::wav::WavFile;
use lofty::mp4::{AtomData, AtomIdent, Ilst, Mp4Codec, Mp4File};
use lofty::mpeg::{Layer, MpegFile};
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OpusFile, VorbisFile};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, ItemValue, Tag, TagType};

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

/// Read one file, with the tag types `types` lists. A panic while parsing it, in lofty or the
/// mapping, is [`ReadError::Malformed`] with a fixed reason, never the panic's message, which
/// may name a host path.
pub fn read(path: &Path, types: &TagTypes) -> Result<ReadResult, ReadError> {
    // The closure borrows only `path` and `types`, and everything the parse builds is dropped as
    // it unwinds, so nothing a panic interrupted is used again.
    panic::catch_unwind(|| parse(path, types))
        .unwrap_or_else(|_| Err(ReadError::Malformed("the tag reader failed unexpectedly".into())))
}

/// [`read`] without the panic guard.
fn parse(path: &Path, types: &TagTypes) -> Result<ReadResult, ReadError> {
    let mut file = File::open(path)?;
    let options = ParseOptions::new();

    // Every format is read by its own reader, whose tags still hold the custom fields lofty's
    // generic tag drops ([`custom_values`]), in the order they are taken from.
    let (tagged, format, custom): (TaggedFile, Format, CustomValues) = if sniff_mp4(&mut file)? {
        read_mp4(&mut file, options, types)?
    } else {
        let probe = Probe::new(&mut file).options(options).guess_file_type()?;
        match probe.file_type() {
            Some(FileType::Mpeg) => {
                // Layers I and II share MP3's frame sync, so only the frame header tells them
                // apart.
                let mpeg =
                    MpegFile::read_from(probe.into_inner(), options).map_err(map_parse_error)?;
                let raw = [mpeg.id3v2().map(Raw::Id3v2), mpeg.ape().map(Raw::Ape)];
                let custom = custom_values(types, &raw);
                match mpeg.properties().layer() {
                    Layer::Layer3 => (mpeg.into(), Format::Mp3, custom),
                    other => return Err(ReadError::Unsupported(format!("MPEG {other:?}"))),
                }
            }
            Some(FileType::Flac) => {
                let flac =
                    FlacFile::read_from(probe.into_inner(), options).map_err(map_parse_error)?;
                let raw = [flac.vorbis_comments().map(Raw::Vorbis), flac.id3v2().map(Raw::Id3v2)];
                let custom = custom_values(types, &raw);
                (flac.into(), Format::Flac, custom)
            }
            Some(FileType::Vorbis) => {
                let ogg =
                    VorbisFile::read_from(probe.into_inner(), options).map_err(map_parse_error)?;
                let custom = custom_values(types, &[Some(Raw::Vorbis(ogg.vorbis_comments()))]);
                (ogg.into(), Format::Vorbis, custom)
            }
            Some(FileType::Opus) => {
                let opus =
                    OpusFile::read_from(probe.into_inner(), options).map_err(map_parse_error)?;
                let custom = custom_values(types, &[Some(Raw::Vorbis(opus.vorbis_comments()))]);
                (opus.into(), Format::Opus, custom)
            }
            Some(FileType::Aac) => {
                let aac =
                    AacFile::read_from(probe.into_inner(), options).map_err(map_parse_error)?;
                let custom = custom_values(types, &[aac.id3v2().map(Raw::Id3v2)]);
                (aac.into(), Format::Aac, custom)
            }
            Some(FileType::Wav) => {
                let wav =
                    WavFile::read_from(probe.into_inner(), options).map_err(map_parse_error)?;
                let custom = custom_values(types, &[wav.id3v2().map(Raw::Id3v2)]);
                (wav.into(), Format::Wav, custom)
            }
            Some(FileType::Aiff) => {
                let aiff =
                    AiffFile::read_from(probe.into_inner(), options).map_err(map_parse_error)?;
                let custom = custom_values(types, &[aiff.id3v2().map(Raw::Id3v2)]);
                (aiff.into(), Format::Aiff, custom)
            }
            // MP4 without an `ftyp` sniff hit; lofty found it anyway.
            Some(FileType::Mp4) => read_mp4(probe.into_inner(), options, types)?,
            // lofty reads what has no reader here, and says why it cannot or what it is.
            _ => {
                let tagged = probe.read().map_err(map_parse_error)?;
                return Err(ReadError::Unsupported(format!("{:?}", tagged.file_type())));
            }
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

    let tags = map_tags(&tagged, types, custom);
    let embedded_art = front_cover(&tagged).and_then(crate::image::describe);
    Ok(ReadResult { tags, properties, embedded_art })
}

/// An MP4 file, read by the typed reader to learn its codec.
fn read_mp4(
    file: &mut File,
    options: ParseOptions,
    types: &TagTypes,
) -> Result<(TaggedFile, Format, CustomValues), ReadError> {
    let mp4 = Mp4File::read_from(file, options).map_err(map_parse_error)?;
    let format = match mp4.properties().codec() {
        Some(Mp4Codec::AAC) => Format::Aac,
        Some(Mp4Codec::ALAC) => Format::Alac,
        other => return Err(ReadError::Unsupported(format!("MP4 codec {other:?}"))),
    };
    let custom = custom_values(types, &[mp4.ilst().map(Raw::Ilst)]);
    Ok((mp4.into(), format, custom))
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
/// lofty key its values come from, if lofty maps one, the custom field that carries them where
/// that finds none, and the generation of types it was added in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TagTypeField {
    key: Cow<'static, str>,
    item: Option<ItemKey>,
    field: Option<Cow<'static, str>>,
    since: u32,
}

/// The tag types built in (`requirements/tags.md` §2). A new one is an entry here, in the
/// generation after the newest, which [`TagTypes::with`] keeps plugins' types above. ID3v2.3
/// has no mood frame, so taggers write mood there as `TXXX:MOOD`.
static BUILT_IN: [TagTypeField; 2] = [
    TagTypeField { key: Cow::Borrowed("genre"), item: Some(ItemKey::Genre), field: None, since: 1 },
    TagTypeField {
        key: Cow::Borrowed("mood"),
        item: Some(ItemKey::Mood),
        field: Some(Cow::Borrowed("MOOD")),
        since: 2,
    },
];

/// A tag type a plugin added, as the store holds it (`requirements/tags.md` §2): its key, the
/// custom field it is read from, and the generation of types it was added in, after every type
/// before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedTagType {
    pub key: String,
    pub field: String,
    pub since: u32,
}

/// The tag types a scan reads, each the same way, by [`strings`] and then [`custom_values`]: the
/// built-in ones, and those plugins added, which the scan takes from the store as it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagTypes(Vec<TagTypeField>);

impl TagTypes {
    /// The built-in types alone.
    pub fn built_in() -> TagTypes {
        TagTypes(BUILT_IN.to_vec())
    }

    /// The built-in types and `added`.
    pub fn with(added: Vec<AddedTagType>) -> TagTypes {
        let added = added.into_iter().map(|t| TagTypeField {
            key: Cow::Owned(t.key),
            item: None,
            field: Some(Cow::Owned(t.field)),
            since: t.since,
        });
        TagTypes(BUILT_IN.iter().cloned().chain(added).collect())
    }

    /// The generation the types are read with: the one the newest was added in. A track read
    /// with an older one is read again, though its file is unchanged, for the types added since
    /// (`design/scanning.md` §2).
    pub fn generation(&self) -> u32 {
        self.0.iter().map(|tag_type| tag_type.since).max().unwrap_or(0)
    }
}

/// The keys of the built-in tag types, each of which `tag_types` must list.
pub fn tag_type_keys() -> impl Iterator<Item = &'static str> {
    BUILT_IN.iter().map(|tag_type| tag_type.key.as_ref())
}

/// The values of each tag type's custom field, by type key, from the first of a file's own tags
/// that has any. lofty's generic tag drops every field it has no key for.
type CustomValues = BTreeMap<String, Vec<String>>;

/// A format's own tag, which keeps the fields lofty's generic tag drops.
enum Raw<'a> {
    Id3v2(&'a Id3v2Tag),
    Vorbis(&'a VorbisComments),
    Ape(&'a ApeTag),
    Ilst(&'a Ilst),
}

impl Raw<'_> {
    /// The values of the custom field `field` under the scheme's convention for a field of one's
    /// own: `TXXX:<field>`, a Vorbis comment or APE item of that name, or
    /// `----:com.apple.iTunes:<field>`, its name matched ignoring case, as taggers differ on it.
    /// Native repeats, null-separated values and semicolons come out the same, as in [`strings`].
    fn values(&self, field: &str) -> Vec<String> {
        let texts: Vec<&str> = match self {
            Raw::Id3v2(tag) => tag
                .into_iter()
                .filter_map(|frame| match frame {
                    Frame::UserText(text) if text.description.eq_ignore_ascii_case(field) => {
                        Some(text.content.as_ref())
                    }
                    _ => None,
                })
                .collect(),
            Raw::Vorbis(tag) => tag.get_all(field).collect(),
            Raw::Ape(tag) => tag
                .into_iter()
                .filter(|item| item.key().eq_ignore_ascii_case(field))
                .filter_map(|item| match item.value() {
                    ItemValue::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect(),
            Raw::Ilst(tag) => tag
                .into_iter()
                .filter(|atom| match atom.ident() {
                    AtomIdent::Freeform { mean, name } => {
                        mean == "com.apple.iTunes" && name.eq_ignore_ascii_case(field)
                    }
                    AtomIdent::Fourcc(_) => false,
                })
                .flat_map(|atom| atom.data())
                .filter_map(|data| match data {
                    AtomData::UTF8(text) | AtomData::UTF16(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect(),
        };
        multi_value::split_all(texts.into_iter().flat_map(|text| text.split('\0')))
    }
}

/// The values of each of `types`' custom fields in `raw`, a file's own tags in the order fields
/// are taken from them, each from the first that has any.
fn custom_values(types: &TagTypes, raw: &[Option<Raw<'_>>]) -> CustomValues {
    let mut out = CustomValues::new();
    for tag_type in &types.0 {
        let Some(field) = &tag_type.field else { continue };
        let values = raw.iter().flatten().map(|raw| raw.values(field)).find(|v| !v.is_empty());
        if let Some(values) = values {
            out.insert(tag_type.key.to_string(), values);
        }
    }
    out
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

/// The canonical [`TagSet`] of `file`'s tags, with values of each of `types`, taken from
/// `custom` where lofty's generic tag carries none.
fn map_tags(file: &TaggedFile, types: &TagTypes, mut custom: CustomValues) -> TagSet {
    let tags = ordered_tags(file);
    let mut out = TagSet::default();
    for tag_type in &types.0 {
        let values = tag_type.item.map(|item| strings(&tags, &item)).unwrap_or_default();
        let values = if values.is_empty() {
            custom.remove(tag_type.key.as_ref()).unwrap_or_default()
        } else {
            values
        };
        out.set_values(&tag_type.key, values);
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
    fn a_custom_field_is_read_under_each_schemes_convention_ignoring_case() {
        use lofty::TextEncoding;
        use lofty::ape::ApeItem;
        use lofty::id3::v2::ExtendedTextFrame;
        use lofty::mp4::Atom;
        let mut id3v2 = Id3v2Tag::new();
        for (description, content) in [("energy", "High\0Calm;high"), ("OTHER", "Loud")] {
            id3v2.insert(Frame::UserText(ExtendedTextFrame::new(
                TextEncoding::UTF8,
                description,
                content,
            )));
        }
        let mut vorbis = VorbisComments::default();
        vorbis.push("Energy".into(), "High".into());
        vorbis.push("ENERGY".into(), "Calm".into());
        let mut ape = ApeTag::new();
        ape.insert(ApeItem::new("energy".into(), ItemValue::Text("High;Calm".into())).unwrap());
        let mut ilst = Ilst::new();
        let freeform = |mean: &'static str, name: &'static str| AtomIdent::Freeform {
            mean: mean.into(),
            name: name.into(),
        };
        ilst.insert(Atom::new(
            freeform("com.apple.iTunes", "Energy"),
            AtomData::UTF8("High".into()),
        ));
        ilst.insert(Atom::new(
            freeform("com.apple.iTunes", "ENERGY"),
            AtomData::UTF8("Calm".into()),
        ));
        ilst.insert(Atom::new(freeform("org.example", "ENERGY"), AtomData::UTF8("Loud".into())));
        for raw in [Raw::Id3v2(&id3v2), Raw::Vorbis(&vorbis), Raw::Ape(&ape), Raw::Ilst(&ilst)] {
            assert_eq!(raw.values("ENERGY"), ["High", "Calm"]);
            assert!(raw.values("TEMPO").is_empty());
        }
    }

    #[test]
    fn a_type_reads_the_first_of_a_files_tags_that_carries_its_field() {
        let types = TagTypes::with(vec![AddedTagType {
            key: "acousticbrainz.energy".into(),
            field: "ENERGY".into(),
            since: 3,
        }]);
        let mut first = VorbisComments::default();
        first.push("MOOD".into(), "Calm".into());
        let mut second = VorbisComments::default();
        second.push("ENERGY".into(), "High".into());
        second.push("MOOD".into(), "Dark".into());
        let raw = [None, Some(Raw::Vorbis(&first)), Some(Raw::Vorbis(&second))];
        let got = custom_values(&types, &raw);
        let want = [("acousticbrainz.energy", "High"), ("mood", "Calm")];
        let want = want.map(|(key, value)| (key.to_owned(), vec![value.to_owned()]));
        assert_eq!(got, CustomValues::from(want));
        assert!(custom_values(&TagTypes::built_in(), &raw[..2]).contains_key("mood"));
    }

    #[test]
    fn the_generation_is_the_newest_types() {
        assert_eq!(TagTypes::built_in().generation(), 2);
        let added = |since| AddedTagType { key: "p.k".into(), field: "K".into(), since };
        assert_eq!(TagTypes::with(vec![added(5)]).generation(), 5);
        assert_eq!(TagTypes::with(Vec::new()), TagTypes::built_in());
    }

    #[test]
    fn packed_numbers() {
        assert_eq!(number_pair("3/12"), (Some(3), Some(12)));
        assert_eq!(number_pair("3"), (Some(3), None));
        assert_eq!(number_pair("0/0"), (None, None));
        assert_eq!(number_pair("x"), (None, None));
    }
}
