//! An album's links, resolved fields, counts, type and loudness (`design/database.md` §3). All
//! but the loudness come from its groups and the heads of its tracks' indexes, so they cost the
//! same however many tracks the album holds; the loudness reads each analyzed track's entry in
//! `tracks_analyzed`.

use jewelcase_scanner::release_kind;
use rusqlite::{OptionalExtension, Result, Transaction, params};

use crate::db::feed::{self, Entity, Op};

/// `album_artists` from the album's first track in album order, and `album_discs` from its
/// groups. Album artists already in place are left alone, so the counts their triggers keep don't
/// churn.
pub(super) fn refresh_album_links(tx: &Transaction<'_>, lib: i64, album: i64) -> Result<()> {
    // Every track of an album shares its artists_key, so any one track's album-artist credits
    // are the album's, and the head of `tracks_by_album_order` is the one read. An album with no
    // track left loses them all.
    let first: Option<i64> = tx
        .prepare_cached(
            "SELECT id FROM tracks WHERE album_id = ?1 \
             ORDER BY disc_number, ifnull(track_number, 9223372036854775807), id LIMIT 1",
        )?
        .query_row([album], |r| r.get(0))
        .optional()?;
    tx.prepare_cached(
        "DELETE FROM album_artists WHERE album_id = ?1 AND (position, artist_id) NOT IN \
         (SELECT position, artist_id FROM track_album_artists WHERE track_id = ?2)",
    )?
    .execute(params![album, first])?;
    tx.prepare_cached(
        "INSERT INTO album_artists (library_id, album_id, position, artist_id) \
         SELECT ?1, ?2, position, artist_id FROM track_album_artists WHERE track_id = ?3 \
         ORDER BY position ON CONFLICT DO NOTHING",
    )?
    .execute(params![lib, album, first])?;
    tx.prepare_cached("DELETE FROM album_discs WHERE album_id = ?1")?.execute([album])?;
    tx.prepare_cached(
        "INSERT INTO album_discs (library_id, album_id, disc_number, track_count, track_total) \
         SELECT ?1, ?2, disc_number, SUM(track_count), NULLIF(MAX(track_total), -1) FROM album_groups \
         WHERE album_id = ?2 GROUP BY disc_number",
    )?
    .execute(params![lib, album])?;
    Ok(())
}

/// The value of `column`, one of `album_groups`' text columns, that most of `album`'s tracks
/// carry, ties broken by bytes so a tie never flips between scans (`design/database.md` §3); none
/// when no track has one.
fn most_used(tx: &Transaction<'_>, column: &str, album: i64) -> Result<Option<String>> {
    tx.prepare_cached(&format!(
        "SELECT {column} FROM album_groups WHERE album_id = ?1 AND {column} <> '' \
         GROUP BY {column} ORDER BY SUM(track_count) DESC, {column} LIMIT 1"
    ))?
    .query_row([album], |r| r.get(0))
    .optional()
}

/// The album's resolved fields and counts from its groups, or its deletion when none is left.
pub(super) fn recompute_album(tx: &Transaction<'_>, lib: i64, album: i64, now: i64) -> Result<()> {
    let (count, available, duration, disc_total, largest_disc_total, disc_count, any_compilation): (
        i64,
        i64,
        i64,
        Option<i64>,
        Option<i64>,
        i64,
        i64,
    ) = tx
        .prepare_cached(
            "SELECT COALESCE(SUM(track_count), 0), COALESCE(SUM(available_count), 0), \
             COALESCE(SUM(duration_us), 0), NULLIF(MAX(disc_total), -1), \
             NULLIF(MAX(track_total), -1), COUNT(DISTINCT disc_number), \
             COALESCE(MAX(compilation), 0) FROM album_groups WHERE album_id = ?1",
        )?
        .query_row([album], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
        })?;
    if count == 0 {
        tx.prepare_cached("DELETE FROM albums WHERE id = ?1")?.execute([album])?;
        feed::record(tx, lib, Entity::Album, album, Op::Delete, now)?;
        return Ok(());
    }

    let title = most_used(tx, "title", album)?;
    // The first sort key among the tracks with that title, or with none when no track has one.
    let sort_key: Option<Vec<u8>> = tx
        .prepare_cached(
            "SELECT MIN(sort_key) FROM album_groups WHERE album_id = ?1 AND title = ifnull(?2, '')",
        )?
        .query_row(params![album, title], |r| r.get(0))?;
    let artist_sort_key: Option<Vec<u8>> = tx
        .prepare_cached(
            "SELECT a.sort_key FROM album_artists aa JOIN artists a ON a.id = aa.artist_id \
             WHERE aa.album_id = ?1 ORDER BY aa.position LIMIT 1",
        )?
        .query_row([album], |r| r.get(0))
        .optional()?;
    let release_date = most_used(tx, "release_date", album)?;
    let release_type = most_used(tx, "release_type", album)?;
    let album_type = album_type(release_type.as_deref(), any_compilation != 0, largest_disc_total);

    // The track total is the spec's `trackTotal`: the sum of the totals of the discs present,
    // which `refresh_album_links` has just written.
    tx.prepare_cached(
        "UPDATE albums SET title = ?2, sort_key = COALESCE(?3, sort_key), \
         artist_sort_key = COALESCE(?4, artist_sort_key), type = ?5, release_date = ?6, \
         disc_total = ?7, \
         track_total = (SELECT SUM(track_total) FROM album_discs WHERE album_id = ?1), \
         disc_count = ?8, track_count = ?9, available_track_count = ?10, duration_us = ?11 \
         WHERE id = ?1",
    )?
    .execute(params![
        album,
        title,
        sort_key,
        artist_sort_key,
        album_type,
        release_date,
        disc_total,
        disc_count,
        count,
        available,
        duration
    ])?;
    recompute_album_loudness(tx, album)?;
    feed::record(tx, lib, Entity::Album, album, Op::Upsert, now)?;
    Ok(())
}

/// `requirements/albums.md` §3: the release-type tag is authoritative where it names a type
/// ([`release_kind`]), the compilation flag overrides size inference, and the default is album.
fn album_type(
    release_type: Option<&str>,
    compilation: bool,
    largest_disc_total: Option<i64>,
) -> &'static str {
    if let Some(kind) = release_type.and_then(release_kind) {
        return kind;
    }
    if compilation {
        return "compilation";
    }
    match largest_disc_total {
        Some(1..=3) => "single",
        Some(4..=6) => "ep",
        _ => "album",
    }
}

/// Album loudness for album-mode normalization: the duration-weighted energy mean of the
/// analyzed tracks, and the loudest peak. It reads `tracks_analyzed`, so it costs the album's
/// analyzed tracks, and nothing for an album with none.
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
    tx.prepare_cached("UPDATE albums SET loudness_lufs = ?2, peak_dbtp = ?3 WHERE id = ?1")?
        .execute(params![album, loudness, peak])?;
    Ok(())
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
