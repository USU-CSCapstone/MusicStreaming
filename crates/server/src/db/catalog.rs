//! Derived data the scanner maintains (`design/database.md` §3).
//!
//! Counts and spellings follow each change by trigger (`0007_catalog_counts.sql`). What is left
//! runs for the entities a batch touched, never for the library: album links and art first (they
//! decide who is an album artist, and what each album offers its artists), then artists (whose
//! changed sort key reaches the albums they head), then albums (whose artist sort key reads
//! artists), then tags.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, Result, Transaction};

use super::feed::{self, Entity, Op};
use album::{recompute_album, refresh_album_links};
use artist::refresh_artist;
use tag::refresh_tag;

pub use album::recompute_album_loudness;

mod album;
mod art;
mod artist;
mod tag;

/// Entities whose derived values may be stale after a batch.
#[derive(Debug, Default)]
pub struct Touched {
    pub albums: HashSet<i64>,
    pub artists: HashSet<i64>,
    pub tags: HashSet<i64>,
}

impl Touched {
    /// Everything track `track_id` of library `lib` currently points at; nothing if the track is
    /// another library's.
    pub fn track(&mut self, tx: &Transaction<'_>, lib: i64, track_id: i64) -> Result<()> {
        if let Some(album) = tx
            .query_row(
                "SELECT album_id FROM tracks WHERE id = ?1 AND library_id = ?2",
                [track_id, lib],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        {
            self.albums.insert(album);
        }
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
        Ok(())
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
    for &album in &touched.albums {
        refresh_album_links(tx, lib, album)?;
        art::album(tx, album)?;
    }
    for &artist in &touched.artists {
        refresh_artist(tx, lib, artist, now)?;
    }
    for &album in &touched.albums {
        recompute_album(tx, lib, album, now)?;
    }
    for &tag in &touched.tags {
        refresh_tag(tx, lib, tag, now)?;
    }
    feed::record(tx, lib, Entity::Library, lib, Op::Upsert, now)
}
