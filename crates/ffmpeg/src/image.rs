//! Decoding an image to a few pixels, for placeholders.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::{Config, Error, Registry};

/// Decoded pixels: `width × height` of them, row by row, four bytes each (RGBA).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

pub(crate) fn decode_image(
    config: &Config,
    registry: &Arc<Registry>,
    path: &Path,
    max_side: u32,
) -> Result<Rgba, Error> {
    // `file:` keeps a path from being read as another protocol, and the `min` keeps a small
    // image from being enlarged. An embedded image's path is its track file, and ffmpeg reads
    // the attached picture from that the same way.
    let mut input = std::ffi::OsString::from("file:");
    input.push(path);
    let scale = format!(
        "scale='min(iw,{max_side})':'min(ih,{max_side})':force_original_aspect_ratio=decrease"
    );
    registry.acquire();
    let child = Command::new(&config.ffmpeg)
        .args(["-nostdin", "-hide_banner", "-v", "error", "-i"])
        .arg(input)
        .args(["-map", "0:v:0", "-frames:v", "1", "-vf", &scale])
        // PAM states its own size, so the output needs no guess at how ffmpeg rounded.
        .args(["-f", "image2pipe", "-c:v", "pam", "-pix_fmt", "rgba", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Spawn { binary: config.ffmpeg.clone(), source: e })?;
    let pid = child.id();
    let stalled = registry.register(pid);
    let out = child.wait_with_output();
    registry.unregister(pid);
    let out = out?;
    if stalled.load(Ordering::SeqCst) {
        return Err(Error::Stalled(config.stall_timeout));
    }
    if !out.status.success() {
        return Err(Error::Failed {
            status: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    parse_pam(&out.stdout)
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
