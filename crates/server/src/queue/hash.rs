//! A list's name and its stored form (`design/queue.md` §2).

use sha2::{Digest, Sha256};

/// The lowercase hex SHA-256 of `tracks` as 8-byte little-endian integers: the bytes
/// [`list_bytes`] gives, hashed without making them.
pub(super) fn list_hash(tracks: &[u64]) -> String {
    let mut hasher = Sha256::new();
    for track in tracks {
        hasher.update(track.to_le_bytes());
    }
    hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `tracks` as `queue_lists.tracks` stores them: 8 bytes each, little-endian.
pub(super) fn list_bytes(tracks: &[u64]) -> Vec<u8> {
    tracks.iter().flat_map(|track| track.to_le_bytes()).collect()
}
