//! A list's name (`design/queue.md` §2).

use sha2::{Digest, Sha256};

/// The lowercase hex SHA-256 of `tracks` as 8-byte little-endian integers, which is what names a
/// list on the server and on a device alike.
pub fn list_hash(tracks: &[u64]) -> String {
    let mut hasher = Sha256::new();
    for track in tracks {
        hasher.update(track.to_le_bytes());
    }
    hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}
