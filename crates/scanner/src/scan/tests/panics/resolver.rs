//! A lofty file type whose reader panics, standing in for a bug in one of lofty's parsers.

use std::io::{Read, Seek};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::error::{FileEncodingError, FileParseError};
use lofty::file::{AudioFile, FileType, TaggedFile};
use lofty::io::FileLike;
use lofty::resolve::FileResolver;
use lofty::tag::{TagSupport, TagType};

/// How a file lofty reads with [`Panics`] begins.
pub(super) const PANICS: &[u8] = b"JEWELCASE PANICS";

/// A file type whose reader panics, registered with lofty so that content starting with
/// [`PANICS`] probes as it. lofty's own probe calls the reader, so the panic unwinds through
/// lofty as a bug in its parser would.
pub(super) enum Panics {}

impl From<Panics> for TaggedFile {
    fn from(file: Panics) -> TaggedFile {
        match file {}
    }
}

/// No [`Panics`] value exists, so only `read_from` is ever called.
impl AudioFile for Panics {
    type Properties = ();

    /// Panics with a message naming a path, which no problem may carry.
    fn read_from<R>(_: &mut R, _: ParseOptions) -> Result<Self, FileParseError>
    where
        R: Read + Seek,
    {
        panic!("index out of bounds in /srv/private/music.flac");
    }

    fn save_to<F: FileLike>(&self, _: &mut F, _: WriteOptions) -> Result<(), FileEncodingError> {
        match *self {}
    }

    fn properties(&self) -> &() {
        match *self {}
    }

    fn contains_tag(&self) -> bool {
        match *self {}
    }

    fn contains_tag_type(&self, _: TagType) -> bool {
        match *self {}
    }
}

impl FileResolver for Panics {
    fn extension() -> Option<&'static str> {
        None
    }

    fn primary_tag_type() -> TagType {
        TagType::Id3v2
    }

    fn tag_support(_: TagType) -> TagSupport {
        TagSupport::Unsupported
    }

    /// [`Panics`] for content that starts with [`PANICS`], so no other test's file is one.
    fn guess(buf: &[u8]) -> Option<FileType> {
        buf.starts_with(PANICS).then_some(FileType::Custom("Panics"))
    }
}
