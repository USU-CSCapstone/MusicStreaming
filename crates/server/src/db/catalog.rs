//! Derived data the scanner maintains (`design/database.md` §3).
//!
//! Counts and spellings follow each change by trigger (`0007_catalog_counts.sql`). What is left
//! runs for the entities a batch touched, never for the library: album links and art first (they
//! decide who is an album artist, and what each album offers its artists), then artists, then
//! albums (whose artist sort key reads artists), then tags.

use std::collections::HashSet;

use jewelcase_core::sort_key;
use rusqlite::{OptionalExtension, Result, Transaction, params};

use super::feed::{self, Entity, Op};

mod art;

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

/// `album_artists` and `album_discs` from the album's tracks. Album artists already in place
/// are left alone, so the counts their triggers keep don't churn.
fn refresh_album_links(tx: &Transaction<'_>, lib: i64, album: i64) -> Result<()> {
    // Every track of an album shares its artists_key, so any one track's album-artist credits
    // are the album's. The lowest id is deterministic.
    tx.execute(
        "DELETE FROM album_artists WHERE album_id = ?1 AND (position, artist_id) NOT IN \
         (SELECT position, artist_id FROM track_album_artists \
          WHERE track_id = (SELECT MIN(id) FROM tracks WHERE album_id = ?1))",
        [album],
    )?;
    tx.execute(
        "INSERT INTO album_artists (library_id, album_id, position, artist_id) \
         SELECT ?1, ?2, position, artist_id FROM track_album_artists \
         WHERE track_id = (SELECT MIN(id) FROM tracks WHERE album_id = ?2) ORDER BY position \
         ON CONFLICT DO NOTHING",
        params![lib, album],
    )?;
    tx.execute("DELETE FROM album_discs WHERE album_id = ?1", [album])?;
    tx.execute(
        "INSERT INTO album_discs (library_id, album_id, disc_number, track_count, track_total) \
         SELECT ?1, ?2, disc_number, COUNT(*), MAX(track_total) FROM tracks WHERE album_id = ?2 GROUP BY disc_number",
        params![lib, album],
    )?;
    Ok(())
}

/// The most-used spelling, ties broken by bytes so a tie never flips
/// between scans (`design/database.md` §3).
fn most_used(tx: &Transaction<'_>, sql: &str, id: i64) -> Result<Option<String>> {
    tx.query_row(sql, [id], |r| r.get::<_, String>(0)).optional()
}

/// The album's resolved fields and counts from its tracks, or its deletion when none is left.
fn recompute_album(tx: &Transaction<'_>, lib: i64, album: i64, now: i64) -> Result<()> {
    let (count, available, duration, disc_total, track_total, disc_count, any_compilation): (
        i64,
        i64,
        i64,
        Option<i64>,
        Option<i64>,
        i64,
        i64,
    ) = tx.query_row(
        "SELECT COUNT(*), COALESCE(SUM(missing_since IS NULL), 0), COALESCE(SUM(duration_us), 0), MAX(disc_total), \
         MAX(track_total), COUNT(DISTINCT disc_number), COALESCE(MAX(compilation), 0) FROM tracks WHERE album_id = ?1",
        [album],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
    )?;
    if count == 0 {
        tx.execute("DELETE FROM albums WHERE id = ?1", [album])?;
        feed::record(tx, lib, Entity::Album, album, Op::Delete, now)?;
        return Ok(());
    }

    let title = most_used(
        tx,
        "SELECT album_title FROM tracks WHERE album_id = ?1 AND album_title IS NOT NULL \
         GROUP BY album_title ORDER BY COUNT(*) DESC, album_title ASC LIMIT 1",
        album,
    )?;
    let sort_key: Option<Vec<u8>> = tx
        .query_row(
            "SELECT album_sort_key FROM tracks WHERE album_id = ?1 AND album_title IS ?2 ORDER BY album_sort_key LIMIT 1",
            params![album, title],
            |r| r.get(0),
        )
        .optional()?;
    let artist_sort_key: Option<Vec<u8>> = tx
        .query_row(
            "SELECT a.sort_key FROM album_artists aa JOIN artists a ON a.id = aa.artist_id \
             WHERE aa.album_id = ?1 ORDER BY aa.position LIMIT 1",
            [album],
            |r| r.get(0),
        )
        .optional()?;
    let release_date = most_used(
        tx,
        "SELECT release_date FROM tracks WHERE album_id = ?1 AND release_date IS NOT NULL \
         GROUP BY release_date ORDER BY COUNT(*) DESC, release_date ASC LIMIT 1",
        album,
    )?;
    let release_type = most_used(
        tx,
        "SELECT release_type FROM tracks WHERE album_id = ?1 AND release_type IS NOT NULL \
         GROUP BY release_type ORDER BY COUNT(*) DESC, release_type ASC LIMIT 1",
        album,
    )?;
    let album_type = album_type(release_type.as_deref(), any_compilation != 0, track_total);

    tx.execute(
        "UPDATE albums SET title = ?2, sort_key = COALESCE(?3, sort_key), artist_sort_key = COALESCE(?4, artist_sort_key), \
         type = ?5, release_date = ?6, disc_total = ?7, track_total = ?8, disc_count = ?9, track_count = ?10, \
         available_track_count = ?11, duration_us = ?12 WHERE id = ?1",
        params![
            album,
            title,
            sort_key,
            artist_sort_key,
            album_type,
            release_date,
            disc_total,
            track_total,
            disc_count,
            count,
            available,
            duration
        ],
    )?;
    recompute_album_loudness(tx, album)?;
    feed::record(tx, lib, Entity::Album, album, Op::Upsert, now)?;
    Ok(())
}

/// `requirements/albums.md` §3: the release-type tag is authoritative, the
/// compilation flag overrides size inference, and the default is album.
fn album_type(
    release_type: Option<&str>,
    compilation: bool,
    track_total: Option<i64>,
) -> &'static str {
    if let Some(rt) = release_type {
        let rt = rt.to_ascii_lowercase();
        if rt.contains("compilation") {
            return "compilation";
        }
        if rt.contains("single") {
            return "single";
        }
        if rt.split(|c: char| !c.is_alphanumeric()).any(|w| w == "ep") {
            return "ep";
        }
        if rt.contains("album") {
            return "album";
        }
    }
    if compilation {
        return "compilation";
    }
    match track_total {
        Some(1..=3) => "single",
        Some(4..=6) => "ep",
        _ => "album",
    }
}

/// Album loudness for album-mode normalization: the duration-weighted
/// energy mean of the analyzed tracks, and the loudest peak.
pub fn recompute_album_loudness(tx: &Transaction<'_>, album: i64) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "SELECT duration_us, loudness_lufs, peak_dbtp FROM tracks WHERE album_id = ?1 AND loudness_lufs IS NOT NULL",
    )?;
    let mut energy = 0.0f64;
    let mut weight = 0.0f64;
    let mut peak: Option<f64> = None;
    for row in stmt.query_map([album], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?, r.get::<_, Option<f64>>(2)?))
    })? {
        let (dur, lufs, p) = row?;
        let w = (dur.max(1)) as f64;
        energy += w * 10f64.powf(lufs / 10.0);
        weight += w;
        if let Some(p) = p {
            peak = Some(peak.map_or(p, |q: f64| q.max(p)));
        }
    }
    let loudness =
        if weight > 0.0 && energy > 0.0 { Some(10.0 * (energy / weight).log10()) } else { None };
    tx.execute(
        "UPDATE albums SET loudness_lufs = ?2, peak_dbtp = ?3 WHERE id = ?1",
        params![album, loudness, peak],
    )?;
    Ok(())
}

/// The artist's displayed name and sort key from the spellings and sort keys its credits count,
/// and its image and biography from its albums ([`art::artist`]), or its deletion when no track
/// credits it. Whether one does is read from the credits themselves, so a wrong count can never
/// delete a credited artist. The unknown artist has no image or biography.
fn refresh_artist(tx: &Transaction<'_>, lib: i64, artist: i64, now: i64) -> Result<()> {
    let Some((credited, name, tagged_sort)) = tx
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM track_artists WHERE artist_id = ?1) \
                 OR EXISTS (SELECT 1 FROM track_album_artists WHERE artist_id = ?1), \
             (SELECT name FROM artist_spellings WHERE artist_id = ?1 ORDER BY uses DESC, name LIMIT 1), \
             (SELECT sort_key FROM artist_sort_keys WHERE artist_id = ?1 ORDER BY uses DESC, sort_key LIMIT 1) \
             FROM artists WHERE id = ?1",
            [artist],
            |r| Ok((r.get::<_, bool>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<Vec<u8>>>(2)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    if !credited {
        tx.execute("DELETE FROM artists WHERE id = ?1", [artist])?;
        return feed::record(tx, lib, Entity::Artist, artist, Op::Delete, now);
    }
    // The tagged sort value where this artist is the primary credit, else derived from the name.
    let sort_key = tagged_sort
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| sort_key(name.as_deref().unwrap_or(""), None).into_bytes());
    tx.execute(
        "UPDATE artists SET name = ?2, sort_key = ?3 WHERE id = ?1",
        params![artist, name, sort_key],
    )?;
    art::artist(tx, artist, name.is_some())?;
    feed::record(tx, lib, Entity::Artist, artist, Op::Upsert, now)
}

/// The tag's displayed name and sort key from the spellings its tracks count, or its deletion
/// when no track carries it. Whether one does is read from the tracks' genres, not the count.
fn refresh_tag(tx: &Transaction<'_>, lib: i64, tag: i64, now: i64) -> Result<()> {
    let Some((carried, name)) = tx
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM track_tags WHERE tag_id = ?1), \
             (SELECT name FROM tag_spellings WHERE tag_id = ?1 ORDER BY uses DESC, name LIMIT 1) \
             FROM tags WHERE id = ?1",
            [tag],
            |r| Ok((r.get::<_, bool>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    if !carried {
        tx.execute("DELETE FROM tags WHERE id = ?1", [tag])?;
        return feed::record(tx, lib, Entity::Tag, tag, Op::Delete, now);
    }
    let name = name.unwrap_or_default();
    tx.execute(
        "UPDATE tags SET name = ?2, sort_key = ?3 WHERE id = ?1",
        params![tag, name, sort_key(&name, None).into_bytes()],
    )?;
    feed::record(tx, lib, Entity::Tag, tag, Op::Upsert, now)
}

#[cfg(test)]
mod tests {
    use super::album_type;

    #[test]
    fn album_types_follow_the_requirement() {
        assert_eq!(album_type(Some("album"), false, Some(3)), "album", "tag beats size");
        assert_eq!(album_type(Some("album; compilation"), false, None), "compilation");
        assert_eq!(album_type(Some("ep"), false, None), "ep");
        assert_eq!(album_type(None, true, Some(12)), "compilation", "flag beats size");
        assert_eq!(album_type(None, false, Some(2)), "single");
        assert_eq!(album_type(None, false, Some(5)), "ep");
        assert_eq!(album_type(None, false, Some(12)), "album");
        assert_eq!(album_type(None, false, None), "album", "no usable total falls through");
    }
}
