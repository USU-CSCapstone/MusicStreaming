//! Image header facts for the `images` table: hash and dimensions.
//! Reads headers only; the placeholder is the placeholder job's (`placeholders.rs`).

use std::panic;

use imagesize::{ImageResult, ImageSize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
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
    use crate::testing::PNG;

    #[test]
    fn describes_png_and_rejects_text() {
        let info = describe(PNG).unwrap();
        assert_eq!((info.width, info.height), (2, 1));
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
