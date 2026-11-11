//! A list's stored form (`design/queue.md` §2).

pub use jewelcase_core::list_hash;

/// `tracks` as `queue_lists.tracks` stores them: 8 bytes each, little-endian.
pub(super) fn list_bytes(tracks: &[u64]) -> Vec<u8> {
    tracks.iter().flat_map(|track| track.to_le_bytes()).collect()
}
