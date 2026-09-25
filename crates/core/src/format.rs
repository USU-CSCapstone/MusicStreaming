//! The supported audio formats (`requirements/scanning.md` §1).
//!
//! This is the single list. The scanner's extension filter, the probe, and the
//! ffmpeg decoder check all derive from it; nothing else enumerates formats.

use serde::{Deserialize, Serialize};

/// One of the eight supported audio formats. WMA is deliberately absent
/// (`requirements/scanning.md` §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    Flac,
    Alac,
    Wav,
    Aiff,
    Mp3,
    Aac,
    Vorbis,
    Opus,
}

impl Format {
    /// Every format, in a stable order.
    pub const ALL: [Format; 8] = [
        Format::Flac,
        Format::Alac,
        Format::Wav,
        Format::Aiff,
        Format::Mp3,
        Format::Aac,
        Format::Vorbis,
        Format::Opus,
    ];

    /// File extensions (lowercase, no dot) that may hold a supported format.
    ///
    /// This is the discovery filter: a path whose extension is not here is
    /// never opened. Several extensions are ambiguous (`.m4a` is AAC or ALAC,
    /// `.ogg` is Vorbis or Opus, `.aac` may be ADTS or raw), so the probe
    /// settles the format from content, not from this list.
    pub const EXTENSIONS: &'static [&'static str] = &[
        "flac", "m4a", "m4b", "mp4", "wav", "wave", "aif", "aiff", "aifc", "mp3", "mp2", "aac",
        "ogg", "oga", "opus",
    ];

    /// Whether a lowercase extension may be a supported format.
    pub fn extension_is_candidate(ext: &str) -> bool {
        Self::EXTENSIONS.contains(&ext)
    }

    /// The name of the ffmpeg decoder this format needs.
    pub fn ffmpeg_decoder(self) -> &'static str {
        match self {
            Format::Flac => "flac",
            Format::Alac => "alac",
            // PCM decoders are per sample width; `verify` checks the common ones.
            Format::Wav | Format::Aiff => "pcm_s16le",
            Format::Mp3 => "mp3",
            Format::Aac => "aac",
            Format::Vorbis => "vorbis",
            Format::Opus => "opus",
        }
    }

    /// The codec name, in ffmpeg's vocabulary. This is what the API's
    /// `codec` strings and the `codecs` query parameter carry.
    pub fn codec_name(self) -> &'static str {
        match self {
            Format::Flac => "flac",
            Format::Alac => "alac",
            Format::Wav | Format::Aiff => "pcm",
            Format::Mp3 => "mp3",
            Format::Aac => "aac",
            Format::Vorbis => "vorbis",
            Format::Opus => "opus",
        }
    }

    /// The container name, in ffmpeg's vocabulary.
    pub fn container_name(self) -> &'static str {
        match self {
            Format::Flac => "flac",
            Format::Alac | Format::Aac => "mp4",
            Format::Wav => "wav",
            Format::Aiff => "aiff",
            Format::Mp3 => "mp3",
            Format::Vorbis | Format::Opus => "ogg",
        }
    }

    /// Whether the format is lossless.
    pub fn is_lossless(self) -> bool {
        matches!(
            self,
            Format::Flac | Format::Alac | Format::Wav | Format::Aiff
        )
    }

    /// Display name for clients.
    pub fn display_name(self) -> &'static str {
        match self {
            Format::Flac => "FLAC",
            Format::Alac => "ALAC",
            Format::Wav => "WAV",
            Format::Aiff => "AIFF",
            Format::Mp3 => "MP3",
            Format::Aac => "AAC",
            Format::Vorbis => "Ogg Vorbis",
            Format::Opus => "Opus",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wma_is_not_a_candidate() {
        assert!(!Format::extension_is_candidate("wma"));
        assert!(!Format::extension_is_candidate("asf"));
    }

    #[test]
    fn every_format_has_a_decoder() {
        for f in Format::ALL {
            assert!(!f.ffmpeg_decoder().is_empty());
        }
    }
}
