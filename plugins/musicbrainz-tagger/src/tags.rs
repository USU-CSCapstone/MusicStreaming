//! Filling in a file's missing tags, in its bytes, without any I/O. Tested natively.
//!
//! Only tags the file is missing are written, so nothing a person set is ever overwritten. The
//! one exception is a title that is not the song's own, such as `Artist - Title (Official
//! Audio)`, which is replaced by the song's. A cover is embedded only in a file with no picture.
//! Several artists are written as one value separated by semicolons, which is how
//! Jewelcase reads them (requirements/artists.md §1).

use std::io::Cursor;

use lofty::config::WriteOptions;
use lofty::file::TaggedFile;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag};

/// What to fill in, from the recording a search matched.
#[derive(Debug, Clone, PartialEq)]
pub struct Tags {
    pub title: String,
    /// Whether to replace the title the file has, because it named the artist too.
    pub retitle: bool,
    /// In order; the first is the primary credit.
    pub artists: Vec<String>,
    pub album_artists: Vec<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    /// `YYYY`, `YYYY-MM`, or `YYYY-MM-DD`.
    pub date: Option<String>,
}

/// A file with tags filled in.
#[derive(Debug)]
pub struct Filled {
    pub bytes: Vec<u8>,
    /// The tags written, by name, for the log.
    pub written: Vec<&'static str>,
}

/// Whether `file`, a whole audio file, has a picture in any of its tags, so a cover need not be
/// fetched for it. An error when it is not a file whose tags can be read.
pub fn has_cover(file: &[u8]) -> Result<bool, String> {
    Ok(probe(file)?.tags().iter().any(|tag| !tag.pictures().is_empty()))
}

/// `file`, a whole audio file, with the tags it is missing filled in from `tags`, in its main
/// tag, which is made if it has none, and `cover`, a JPEG or PNG image, as its front cover if
/// it has no picture. None when it is missing none of them, so there is nothing to write. An
/// error when it is not a file whose tags can be read or written.
pub fn fill(file: Vec<u8>, tags: &Tags, cover: Option<Vec<u8>>) -> Result<Option<Filled>, String> {
    let tagged = probe(&file)?;
    let kind = tagged.primary_tag_type();
    let has_picture = tagged.tags().iter().any(|tag| !tag.pictures().is_empty());
    let mut tag = tagged.tag(kind).cloned().unwrap_or_else(|| Tag::new(kind));
    drop(tagged);
    let mut written = fill_tag(&mut tag, tags);
    if let Some(cover) = cover.filter(|_| !has_picture)
        && let Some(mime) = image_type(&cover)
    {
        let picture = Picture::unchecked(cover).pic_type(PictureType::CoverFront);
        tag.push_picture(picture.mime_type(mime).build());
        written.push("cover");
    }
    if written.is_empty() {
        return Ok(None);
    }
    let mut cursor = Cursor::new(file);
    tag.save_to(&mut cursor, WriteOptions::default())
        .map_err(|e| format!("its tags could not be written: {e}"))?;
    Ok(Some(Filled { bytes: cursor.into_inner(), written }))
}

/// The tags in `file`, a whole audio file.
fn probe(file: &[u8]) -> Result<TaggedFile, String> {
    Probe::new(Cursor::new(file))
        .guess_file_type()
        .map_err(|e| format!("its tags could not be read: {e}"))?
        .read()
        .map_err(|e| format!("its tags could not be read: {e}"))
}

/// What kind of image `image` is, from its first bytes: JPEG or PNG, the kinds every tag
/// format holds, or none.
fn image_type(image: &[u8]) -> Option<MimeType> {
    if image.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(MimeType::Jpeg)
    } else if image.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(MimeType::Png)
    } else {
        None
    }
}

/// Fills in what `tag` is missing from `tags`, and answers the names of what it filled in.
fn fill_tag(tag: &mut Tag, tags: &Tags) -> Vec<&'static str> {
    let mut written = Vec::new();
    let mut set = |tag: &mut Tag, key: ItemKey, name, value: Option<String>, replace: bool| {
        let Some(value) = value.filter(|v| !v.trim().is_empty()) else { return };
        let missing = tag.get_string(key).is_none_or(|v| v.trim().is_empty());
        if (missing || replace) && tag.get_string(key) != Some(value.as_str()) {
            tag.insert_text(key, value);
            written.push(name);
        }
    };
    let joined = |names: &[String]| Some(names.join("; "));
    set(tag, ItemKey::TrackTitle, "title", Some(tags.title.clone()), tags.retitle);
    set(tag, ItemKey::TrackArtist, "artist", joined(&tags.artists), false);
    set(tag, ItemKey::AlbumArtist, "album artist", joined(&tags.album_artists), false);
    set(tag, ItemKey::AlbumTitle, "album", tags.album.clone(), false);
    set(tag, ItemKey::TrackNumber, "track number", tags.track.map(|n| n.to_string()), false);
    set(tag, ItemKey::DiscNumber, "disc number", tags.disc.map(|n| n.to_string()), false);
    set(tag, ItemKey::RecordingDate, "date", tags.date.clone(), false);
    written
}

#[cfg(test)]
mod tests;
