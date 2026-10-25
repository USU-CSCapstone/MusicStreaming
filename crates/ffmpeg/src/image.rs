//! Decoding an image scaled to fit a size: to a few pixels for placeholders, or to JPEG for the
//! server's artwork.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use crate::pooled::Pooled;
use crate::{Config, Error, Registry, input};

/// Decoded pixels: `width × height` of them, row by row, four bytes each (RGBA).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// PAM of RGBA pixels. PAM states its own size, so the output needs no guess at how ffmpeg
/// rounded.
const PAM: [&str; 4] = ["-c:v", "pam", "-pix_fmt", "rgba"];

/// JPEG, at a quality that suits artwork.
pub(crate) const JPEG: [&str; 4] = ["-c:v", "mjpeg", "-q:v", "3"];

/// The command that decodes the image at `path`, or the picture attached to an audio file,
/// scales it to fit `max_side` without enlarging it, and writes it to its standard output,
/// encoded by `codec` ([`JPEG`] or the PAM [`decode_image`] reads). It reads no input. The caller
/// pipes its output.
pub(crate) fn command(config: &Config, path: &Path, max_side: u32, codec: [&str; 4]) -> Command {
    // The `min` keeps a small image from being enlarged.
    let scale = format!(
        "scale='min(iw,{max_side})':'min(ih,{max_side})':force_original_aspect_ratio=decrease"
    );
    let mut command = Command::new(&config.ffmpeg);
    command
        .args(["-nostdin", "-hide_banner", "-v", "error"])
        .args(input(path))
        .args(["-map", "0:v:0", "-frames:v", "1", "-vf", &scale, "-f", "image2pipe"])
        .args(codec)
        .arg("pipe:1")
        .stdin(Stdio::null());
    command
}

/// Decodes the image at `path` to fit `max_side`; see [`crate::Ffmpeg::decode_image`].
pub(crate) fn decode_image(
    config: &Config,
    registry: &Arc<Registry>,
    path: &Path,
    max_side: u32,
) -> Result<Rgba, Error> {
    let out = Pooled::spawn(registry, &mut command(config, path, max_side, PAM))?.output()?;
    parse_pam(&out)
        .filter(|image| image.width <= max_side as usize && image.height <= max_side as usize)
        .ok_or_else(|| Error::Image("ffmpeg's output was not the expected PAM".into()))
}

/// Reads a PAM image of RGBA pixels: a text header of `KEY value` lines ending at `ENDHDR`,
/// then the pixels.
fn parse_pam(bytes: &[u8]) -> Option<Rgba> {
    const END: &[u8] = b"ENDHDR\n";
    let end = bytes.windows(END.len()).position(|window| window == END)?;
    let header = std::str::from_utf8(&bytes[..end]).ok()?;
    let mut lines = header.lines();
    if lines.next()? != "P7" {
        return None;
    }
    let (mut width, mut height, mut depth, mut maxval): (usize, usize, u32, u32) = (0, 0, 0, 0);
    for line in lines {
        let (key, value) = line.split_once(' ')?;
        let value = value.trim();
        match key {
            "WIDTH" => width = value.parse().ok()?,
            "HEIGHT" => height = value.parse().ok()?,
            "DEPTH" => depth = value.parse().ok()?,
            "MAXVAL" => maxval = value.parse().ok()?,
            _ => {}
        }
    }
    let pixels = &bytes[end + END.len()..];
    let size = width.checked_mul(height)?.checked_mul(4)?;
    (width > 0 && height > 0 && depth == 4 && maxval == 255 && pixels.len() == size).then(|| Rgba {
        width,
        height,
        pixels: pixels.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pam() {
        let mut pam =
            b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();
        pam.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            parse_pam(&pam),
            Some(Rgba { width: 2, height: 1, pixels: vec![1, 2, 3, 4, 5, 6, 7, 8] })
        );
        // Too few pixels for the size it states.
        assert_eq!(parse_pam(&pam[..pam.len() - 1]), None);
        assert_eq!(parse_pam(b"P6\n1 1\n255\n"), None);
    }
}
