//! Derived data the scanner maintains (`design/database.md` §3).
//!
//! Counts, spellings and album groups follow each change by trigger (`0007_catalog_counts.sql`).
//! What is left runs for the entities a batch touched, never for the library, and reads counts,
//! groups and the heads of indexes rather than every track: album links first (they decide who is
//! an album artist, which an artist's image and biography read), then album art, then artists, then
//! albums, then the artwork of the playlists holding a track moved onto another album or a track
//! of an album whose art changed, then tags. Artists before albums is not needed: an album reads
//! its first artist's sort key as it stands, and an artist whose key changes passes it on to every
//! album it heads, so either order ends with the same keys.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, Result, Transaction};

use super::feed::{self, Entity, Op};
use album::{recompute_album, refresh_album_links};
use artist::refresh_artist;
use tag::refresh_tag;

pub use album::recompute_album_loudness;
pub use art::mosaic_id;
#[cfg(test)]
pub use art::playlist as playlist_artwork;

mod album;
mod art;
mod artist;
mod tag;

/// Entities whose derived values may be stale after a batch.
#[derive(Debug, Default)]
pub struct Touched {
    /// Tracks moved onto another album, whose playlists' artwork may follow them.
    pub moved: HashSet<i64>,
    pub albums: HashSet<i64>,
    pub artists: HashSet<i64>,
    pub tags: HashSet<i64>,
}

impl Touched {
    /// Everything track `track_id` of library `lib` currently points at; nothing if the track is
    /// another library's. Returns its album, if it has one in `lib`.
    pub fn track(&mut self, tx: &Transaction<'_>, lib: i64, track_id: i64) -> Result<Option<i64>> {
        let album = tx
            .prepare_cached("SELECT album_id FROM tracks WHERE id = ?1 AND library_id = ?2")?
            .query_row([track_id, lib], |r| r.get::<_, i64>(0))
            .optional()?;
        self.albums.extend(album);
        let mut stmt = tx.prepare_cached(
            "SELECT artist_id FROM track_artists WHERE track_id = ?1 AND library_id = ?2 \
             UNION SELECT artist_id FROM track_album_artists WHERE track_id = ?1 AND library_id = ?2",
        )?;
        for id in stmt.query_map([track_id, lib], |r| r.get::<_, i64>(0))? {
            self.artists.insert(id?);
        }
        let mut stmt = tx.prepare_cached(
            "SELECT tag_id FROM track_tags WHERE track_id = ?1 AND library_id = ?2",
        )?;
        for id in stmt.query_map([track_id, lib], |r| r.get::<_, i64>(0))? {
            self.tags.insert(id?);
        }
        Ok(album)
    }
}

/// Brings what `touched` names up to date and records each change, and the library's, in the
/// feed at `now`. An empty `touched` changes nothing. `touched` needs only what the changed
/// tracks pointed at before and after, since every album, artist and tag a change reaches is one
/// of those.
pub fn recompute(tx: &Transaction<'_>, lib: i64, touched: &Touched, now: i64) -> Result<()> {
    if touched.albums.is_empty() && touched.artists.is_empty() && touched.tags.is_empty() {
        return Ok(());
    }
    let mut art_changed = Vec::new();
    for &album in &touched.albums {
        refresh_album_links(tx, lib, album)?;
        if art::album(tx, album)? {
            art_changed.push(album);
        }
    }
    for &artist in &touched.artists {
        refresh_artist(tx, lib, artist, now)?;
    }
    for &album in &touched.albums {
        recompute_album(tx, lib, album, now)?;
    }
    art::playlists(tx, lib, &touched.moved, &art_changed)?;
    for &tag in &touched.tags {
        refresh_tag(tx, lib, tag, now)?;
    }
    feed::record(tx, lib, Entity::Library, lib, Op::Upsert, now)
}
