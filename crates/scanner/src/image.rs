//! Image header facts for the `images` table: hash and dimensions.
//! Reads headers only; the placeholder is the placeholder job's (`placeholders.rs`).

use std::panic;

use imagesize::{ImageResult, ImageSize};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    /// SHA-256 of the bytes.
    pub hash: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Describe image bytes. `None` when they are not a recognizable image, or the
/// header reader panics on them, in which case the file is ignored as artwork
/// rather than reported.
pub fn describe(bytes: &[u8]) -> Option<ImageInfo> {
    describe_with(bytes, imagesize::blob_size)
}

/// [`describe`], reading the header with `read_size`, so a test can pass in one that panics.
fn describe_with(
    bytes: &[u8],
    read_size: fn(&[u8]) -> ImageResult<ImageSize>,
) -> Option<ImageInfo> {
    // The closure borrows only `bytes`, which a panic cannot change.
    let Ok(Ok(size)) = panic::catch_unwind(|| read_size(bytes)) else {
        return None;
    };
    let hash = Sha256::digest(bytes).to_vec();
    Some(ImageInfo { hash, width: size.width as u32, height: size.height as u32 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1x1 PNG.
    pub const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn describes_png_and_rejects_text() {
        let info = describe(PNG).unwrap();
        assert_eq!((info.width, info.height), (1, 1));
        assert_eq!(info.hash.len(), 32);
        assert!(describe(b"not an image").is_none());
    }

    /// A header reader that panics leaves the image undescribed, as an unrecognizable one is. No
    /// real input is known to panic imagesize, so the test passes in a reader that does.
    #[test]
    fn a_panicking_header_reader_describes_nothing() {
        assert!(describe_with(PNG, |_| panic!("a header reader bug")).is_none());
    }
}
