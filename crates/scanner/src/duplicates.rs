//! Duplicate detection (`requirements/scanning.md` §9, `design/scanning.md` §14).
//!
//! A background job over the index, never part of a scan. Informational
//! only: nothing here touches a track row.

use std::collections::BTreeMap;

use crate::store::Store;
use crate::types::*;

/// Find likely duplicates. Content matching awaits track identity.
pub fn find_duplicates(store: &dyn Store, library: &LibraryId) -> Vec<DuplicateGroup> {
    let candidates = store.duplicate_candidates(library);
    let mut groups = Vec::new();

    // Matching tags: title, artists, album, album artists, disc, track.
    let mut by_tags: BTreeMap<String, Vec<&DuplicateCandidate>> = BTreeMap::new();
    for c in &candidates {
        let Some(title) = &c.title else { continue };
        let key = format!(
            "{}\u{1}{}\u{1}{}\u{1}{}\u{1}{:?}\u{1}{:?}",
            title.to_lowercase(),
            c.artists.join(";").to_lowercase(),
            c.album.as_deref().unwrap_or("").to_lowercase(),
            c.album_artists.join(";").to_lowercase(),
            c.disc_number,
            c.track_number
        );
        by_tags.entry(key).or_default().push(c);
    }
    for (_, members) in by_tags {
        if members.len() > 1 {
            groups.push(DuplicateGroup {
                reason: DuplicateReason::MatchingTags,
                tracks: members
                    .iter()
                    .map(|c| (c.track_id, c.path.clone()))
                    .collect(),
            });
        }
    }

    // One album under two paths: same album identity spanning directories.
    let mut by_album: BTreeMap<String, BTreeMap<std::path::PathBuf, Vec<&DuplicateCandidate>>> =
        BTreeMap::new();
    for c in &candidates {
        let Some(album) = &c.album else { continue };
        let key = format!(
            "{}\u{1}{}",
            album.to_lowercase(),
            c.album_artists.join(";").to_lowercase()
        );
        let dir = c.path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        by_album
            .entry(key)
            .or_default()
            .entry(dir)
            .or_default()
            .push(c);
    }
    for (_, dirs) in by_album {
        if dirs.len() > 1 {
            groups.push(DuplicateGroup {
                reason: DuplicateReason::AlbumUnderTwoPaths,
                tracks: dirs
                    .values()
                    .flatten()
                    .map(|c| (c.track_id, c.path.clone()))
                    .collect(),
            });
        }
    }
    groups
}
