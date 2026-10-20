//! How every ffmpeg and ffprobe invocation names its input.

use std::ffi::OsString;
use std::path::Path;

/// The most pixels a picture may have: 8192², about 64 megapixels, which admits a 600-dpi scan
/// of an LP sleeve. Decoding costs about 6 bytes a pixel, so a small file that declares a huge
/// picture would otherwise cost gigabytes of memory.
const MAX_PIXELS: u32 = 8192 * 8192;

/// The arguments that open `path` as the input, placed where `-i <path>` would go.
///
/// The `file:` prefix keeps a path that starts with `-` or looks like a URL from being read as
/// anything but a file, and the whitelist keeps a container that names further inputs, such as
/// an HLS playlist, to local files too. A name containing `%d` may still be read as an image
/// sequence.
///
/// A picture larger than 8192² pixels is refused before its pixels are allocated. That covers
/// an audio file's attached picture, which ffprobe and an audio decode read too while they look
/// at the streams: there the picture is skipped and the audio still decodes, while an image
/// decode exits with an error.
pub(crate) fn input(path: &Path) -> [OsString; 6] {
    let mut file = OsString::from("file:");
    file.push(path);
    let max_pixels = MAX_PIXELS.to_string().into();
    [
        "-max_pixels".into(),
        max_pixels,
        "-protocol_whitelist".into(),
        "file".into(),
        "-i".into(),
        file,
    ]
}
