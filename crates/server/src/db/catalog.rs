//! Derived data the scanner maintains (`design/database.md` §3).
//!
//! Recomputed for the entities a batch touched, never for the library.
//! Order matters: album links first (they decide who is an album artist),
//! then artists, then albums (whose artist sort key reads artists), then
//! tags, then library totals.

use std::collections::HashSet;

use jewelcase_core::sort;
use rusqlite::{OptionalExtension, Transaction, params};

use super::Result;
use super::feed::{self, Entity, Op};

/// Entities whose derived values may be stale after a batch.
#[derive(Debug, Default)]
pub struct Touched {
    pub albums: HashSet<i64>,
    pub artists: HashSet<i64>,
    pub tags: HashSet<i64>,
}

impl Touched {
    /// Everything a track currently points at.
    pub fn track(&mut self, tx: &Transaction<'_>, track_id: i64) -> Result<()> {
        if let Some(album) = tx
            .query_row(
                "SELECT album_id FROM tracks WHERE id = ?1",
                [track_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        {
            self.albums.insert(album);
        }
        let mut stmt = tx.prepare_cached(
            "SELECT artist_id FROM track_artists WHERE track_id = ?1 UNION SELECT artist_id FROM track_album_artists WHERE track_id = ?1",
        )?;
        for id in stmt.query_map([track_id], |r| r.get::<_, i64>(0))? {
            self.artists.insert(id?);
        }
        let mut stmt = tx.prepare_cached("SELECT tag_id FROM track_tags WHERE track_id = ?1")?;
        for id in stmt.query_map([track_id], |r| r.get::<_, i64>(0))? {
            self.tags.insert(id?);
        }
        Ok(())
    }
}

pub fn recompute(tx: &Transaction<'_>, lib: i64, touched: &mut Touched, now: i64) -> Result<()> {
    if touched.albums.is_empty() && touched.artists.is_empty() && touched.tags.is_empty() {
        return Ok(());
    }
    // 1. Album links. Old and new album artists and tags become touched.
    for album in touched.albums.clone() {
        refresh_album_links(tx, lib, album, touched)?;
    }
    // 2. Artists.
    for artist in touched.artists.clone() {
        recompute_artist(tx, lib, artist, touched, now)?;
    }
    // 3. Albums.
    for album in touched.albums.clone() {
        recompute_album(tx, lib, album, now)?;
    }
    // 4. Tags.
    for tag in touched.tags.clone() {
        recompute_tag(tx, lib, tag, now)?;
    }
    // 5. The library.
    tx.execute(
        "UPDATE libraries SET \
         track_count = (SELECT COUNT(*) FROM tracks WHERE library_id = ?1), \
         album_count = (SELECT COUNT(*) FROM albums WHERE library_id = ?1), \
         artist_count = (SELECT COUNT(*) FROM artists WHERE library_id = ?1), \
         duration_us = (SELECT COALESCE(SUM(duration_us), 0) FROM tracks WHERE library_id = ?1), \
         updated_at = ?2 WHERE id = ?1",
        params![lib, now],
    )?;
    feed::record(tx, lib, Entity::Library, lib, Op::Upsert, now)?;
    Ok(())
}

/// `album_artists`, `album_discs`, and `album_tags` from the album's tracks.
fn refresh_album_links(
    tx: &Transaction<'_>,
    lib: i64,
    album: i64,
    touched: &mut Touched,
) -> Result<()> {
    // Previous album artists and tags may lose this album.
    let mut stmt = tx.prepare_cached("SELECT artist_id FROM album_artists WHERE album_id = ?1")?;
    for id in stmt.query_map([album], |r| r.get::<_, i64>(0))? {
        touched.artists.insert(id?);
    }
    let mut stmt = tx.prepare_cached("SELECT tag_id FROM album_tags WHERE album_id = ?1")?;
    for id in stmt.query_map([album], |r| r.get::<_, i64>(0))? {
        touched.tags.insert(id?);
    }

    tx.execute("DELETE FROM album_artists WHERE album_id = ?1", [album])?;
    tx.execute("DELETE FROM album_discs WHERE album_id = ?1", [album])?;
    tx.execute("DELETE FROM album_tags WHERE album_id = ?1", [album])?;

    // Every track of an album shares its artists_key, so any one track's
    // album-artist credits are the album's. The lowest id is deterministic.
    tx.execute(
        "INSERT OR IGNORE INTO album_artists (library_id, album_id, position, artist_id) \
         SELECT ?1, ?2, position, artist_id FROM track_album_artists \
         WHERE track_id = (SELECT MIN(id) FROM tracks WHERE album_id = ?2) ORDER BY position",
        params![lib, album],
    )?;
    tx.execute(
        "INSERT INTO album_discs (library_id, album_id, disc_number, track_count, track_total) \
         SELECT ?1, ?2, disc_number, COUNT(*), MAX(track_total) FROM tracks WHERE album_id = ?2 GROUP BY disc_number",
        params![lib, album],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO album_tags (library_id, album_id, tag_id) \
         SELECT DISTINCT ?1, ?2, tt.tag_id FROM track_tags tt JOIN tracks t ON t.id = tt.track_id WHERE t.album_id = ?2",
        params![lib, album],
    )?;

    let mut stmt = tx.prepare_cached("SELECT artist_id FROM album_artists WHERE album_id = ?1")?;
    for id in stmt.query_map([album], |r| r.get::<_, i64>(0))? {
        touched.artists.insert(id?);
    }
    let mut stmt = tx.prepare_cached("SELECT tag_id FROM album_tags WHERE album_id = ?1")?;
    for id in stmt.query_map([album], |r| r.get::<_, i64>(0))? {
        touched.tags.insert(id?);
    }
    Ok(())
}

/// The most-used spelling, ties broken by bytes so a tie never flips
/// between scans (`design/database.md` §3).
fn most_used(tx: &Transaction<'_>, sql: &str, id: i64) -> Result<Option<String>> {
    Ok(tx
        .query_row(sql, [id], |r| r.get::<_, String>(0))
        .optional()?)
}

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
         available_track_count = ?11, duration_us = ?12, updated_at = ?13 WHERE id = ?1",
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
            duration,
            now
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
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, f64>(1)?,
            r.get::<_, Option<f64>>(2)?,
        ))
    })? {
        let (dur, lufs, p) = row?;
        let w = (dur.max(1)) as f64;
        energy += w * 10f64.powf(lufs / 10.0);
        weight += w;
        if let Some(p) = p {
            peak = Some(peak.map_or(p, |q: f64| q.max(p)));
        }
    }
    let loudness = if weight > 0.0 && energy > 0.0 {
        Some(10.0 * (energy / weight).log10())
    } else {
        None
    };
    tx.execute(
        "UPDATE albums SET loudness_lufs = ?2, peak_dbtp = ?3 WHERE id = ?1",
        params![album, loudness, peak],
    )?;
    Ok(())
}

fn recompute_artist(
    tx: &Transaction<'_>,
    lib: i64,
    artist: i64,
    touched: &mut Touched,
    now: i64,
) -> Result<()> {
    let (track_count, available): (i64, i64) = tx.query_row(
        "SELECT COUNT(DISTINCT ta.track_id), COALESCE(SUM(t.missing_since IS NULL), 0) \
         FROM track_artists ta JOIN tracks t ON t.id = ta.track_id WHERE ta.artist_id = ?1",
        [artist],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let album_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM album_artists WHERE artist_id = ?1",
        [artist],
        |r| r.get(0),
    )?;
    let credited_as_album_artist: i64 = tx.query_row(
        "SELECT COUNT(*) FROM track_album_artists WHERE artist_id = ?1",
        [artist],
        |r| r.get(0),
    )?;
    if track_count == 0 && album_count == 0 && credited_as_album_artist == 0 {
        // Previous tags of this artist may lose their artist count.
        let mut stmt = tx.prepare_cached("SELECT tag_id FROM artist_tags WHERE artist_id = ?1")?;
        for id in stmt.query_map([artist], |r| r.get::<_, i64>(0))? {
            touched.tags.insert(id?);
        }
        tx.execute("DELETE FROM artists WHERE id = ?1", [artist])?;
        feed::record(tx, lib, Entity::Artist, artist, Op::Delete, now)?;
        return Ok(());
    }

    let name = most_used(
        tx,
        "SELECT artist_name FROM (SELECT artist_name FROM track_artists WHERE artist_id = ?1 \
         UNION ALL SELECT artist_name FROM track_album_artists WHERE artist_id = ?1) \
         WHERE artist_name IS NOT NULL GROUP BY artist_name ORDER BY COUNT(*) DESC, artist_name ASC LIMIT 1",
        artist,
    )?;
    // Appearances: credited on a track whose album does not list them
    // (`requirements/artists.md` §2).
    let appearances: i64 = tx.query_row(
        "SELECT COUNT(*) FROM track_artists ta JOIN tracks t ON t.id = ta.track_id WHERE ta.artist_id = ?1 \
         AND NOT EXISTS (SELECT 1 FROM album_artists aa WHERE aa.album_id = t.album_id AND aa.artist_id = ta.artist_id)",
        [artist],
        |r| r.get(0),
    )?;
    // Sort key: the tagged sort value where this artist is the primary
    // credit, else derived from the name.
    let tagged_sort: Option<Vec<u8>> = tx
        .query_row(
            "SELECT t.artist_sort_key FROM track_artists ta JOIN tracks t ON t.id = ta.track_id \
             WHERE ta.artist_id = ?1 AND ta.position = 0 GROUP BY t.artist_sort_key \
             ORDER BY COUNT(*) DESC, t.artist_sort_key ASC LIMIT 1",
            [artist],
            |r| r.get(0),
        )
        .optional()?;
    let sort_key = tagged_sort
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| sort::sort_key(name.as_deref().unwrap_or(""), None).into_bytes());

    let mut stmt = tx.prepare_cached("SELECT tag_id FROM artist_tags WHERE artist_id = ?1")?;
    for id in stmt.query_map([artist], |r| r.get::<_, i64>(0))? {
        touched.tags.insert(id?);
    }
    tx.execute("DELETE FROM artist_tags WHERE artist_id = ?1", [artist])?;
    tx.execute(
        "INSERT OR IGNORE INTO artist_tags (library_id, artist_id, tag_id) \
         SELECT DISTINCT ?1, ?2, tt.tag_id FROM track_tags tt JOIN track_artists ta ON ta.track_id = tt.track_id WHERE ta.artist_id = ?2",
        params![lib, artist],
    )?;
    let mut stmt = tx.prepare_cached("SELECT tag_id FROM artist_tags WHERE artist_id = ?1")?;
    for id in stmt.query_map([artist], |r| r.get::<_, i64>(0))? {
        touched.tags.insert(id?);
    }

    tx.execute(
        "UPDATE artists SET name = ?2, sort_key = ?3, album_count = ?4, track_count = ?5, appearance_count = ?6, \
         available_track_count = ?7, updated_at = ?8 WHERE id = ?1",
        params![artist, name, sort_key, album_count, track_count, appearances, available, now],
    )?;
    feed::record(tx, lib, Entity::Artist, artist, Op::Upsert, now)?;
    Ok(())
}

fn recompute_tag(tx: &Transaction<'_>, lib: i64, tag: i64, now: i64) -> Result<()> {
    let track_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM track_tags WHERE tag_id = ?1",
        [tag],
        |r| r.get(0),
    )?;
    if track_count == 0 {
        tx.execute("DELETE FROM tags WHERE id = ?1", [tag])?;
        feed::record(tx, lib, Entity::Tag, tag, Op::Delete, now)?;
        return Ok(());
    }
    let name = most_used(
        tx,
        "SELECT tag_name FROM track_tags WHERE tag_id = ?1 GROUP BY tag_name ORDER BY COUNT(*) DESC, tag_name ASC LIMIT 1",
        tag,
    )?
    .unwrap_or_default();
    let album_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM album_tags WHERE tag_id = ?1",
        [tag],
        |r| r.get(0),
    )?;
    let artist_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM artist_tags WHERE tag_id = ?1",
        [tag],
        |r| r.get(0),
    )?;
    tx.execute(
        "UPDATE tags SET name = ?2, sort_key = ?3, track_count = ?4, album_count = ?5, artist_count = ?6 WHERE id = ?1",
        params![tag, name, sort::sort_key(&name, None).into_bytes(), track_count, album_count, artist_count],
    )?;
    feed::record(tx, lib, Entity::Tag, tag, Op::Upsert, now)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::album_type;

    #[test]
    fn album_types_follow_the_requirement() {
        assert_eq!(
            album_type(Some("album"), false, Some(3)),
            "album",
            "tag beats size"
        );
        assert_eq!(
            album_type(Some("album; compilation"), false, None),
            "compilation"
        );
        assert_eq!(album_type(Some("ep"), false, None), "ep");
        assert_eq!(
            album_type(None, true, Some(12)),
            "compilation",
            "flag beats size"
        );
        assert_eq!(album_type(None, false, Some(2)), "single");
        assert_eq!(album_type(None, false, Some(5)), "ep");
        assert_eq!(album_type(None, false, Some(12)), "album");
        assert_eq!(
            album_type(None, false, None),
            "album",
            "no usable total falls through"
        );
    }
}
