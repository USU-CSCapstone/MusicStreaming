//! The catalog's part of the sweep: tracks missing for longer than the window, once a later scan
//! has found them still missing (`requirements/scanning.md` §8), and the art and biographies
//! nothing points at.

use rusqlite::{Result, Transaction, params};

use super::{CHUNK, WINDOW_MS, libraries};
use crate::db::catalog::{self, Touched};
use crate::db::feed::{self, Entity, Op};

/// Whether image `r` is used: as a track's art or artist image, or an album's or artist's image.
/// Albums and artists take theirs from their tracks, but a delete must never break their foreign
/// keys.
const IMAGE_USED: &str = "EXISTS (SELECT 1 FROM tracks t \
                              WHERE t.library_id = r.library_id AND t.image_id = r.id) \
                          OR EXISTS (SELECT 1 FROM tracks t \
                              WHERE t.library_id = r.library_id AND t.artist_image_id = r.id) \
                          OR EXISTS (SELECT 1 FROM albums a \
                              WHERE a.library_id = r.library_id AND a.image_id = r.id) \
                          OR EXISTS (SELECT 1 FROM artists a \
                              WHERE a.library_id = r.library_id AND a.image_id = r.id)";

/// Whether biography `r` is a track's. Artists keep a copy of the text, not a reference.
const BIOGRAPHY_USED: &str = "EXISTS (SELECT 1 FROM tracks t \
                                  WHERE t.library_id = r.library_id AND t.biography_id = r.id)";

/// Up to `?3` tracks of library `?1` missing since before `?2`, and since more than `?4` before
/// the start of the last completed scan of their whole root, so none under a root no such scan
/// has walked; read root by root through `tracks_missing`.
pub(super) const MISSING: &str = "SELECT t.id FROM library_roots r JOIN tracks t ON t.root_id = r.id \
     WHERE r.library_id = ?1 AND t.missing_since < min(?2, r.walked_at - ?4) LIMIT ?3";

/// The most tracks one chunk purges: half a scan batch (`design/scanning.md` §7).
/// A track costs about 1 ms with its recompute in a release build, more on a Pi and on a large
/// album, so a step stops close to its time (`design/database.md` §9).
pub(super) const TRACK_CHUNK: usize = 16;

/// Purges up to [`TRACK_CHUNK`] tracks missing for longer than the window and still missing after
/// a completed scan of their whole root that started once the window was up, recording each in
/// the feed, and brings the albums, artists and tags they counted toward up to date in the same
/// transaction, which deletes those they were the last of and records that too.
/// A play of a purged track keeps its snapshot, since `plays.track_id` has no foreign key.
///
/// Returns whether none is left. Playlists are not modelled yet (`design/database.md` §10); a
/// track in one is to be kept.
pub(super) fn tracks(tx: &Transaction<'_>, now: i64, _: &mut i64) -> Result<bool> {
    let mut missing = tx.prepare_cached(MISSING)?;
    let mut delete = tx.prepare_cached("DELETE FROM tracks WHERE id = ?1")?;
    let mut purged = 0;
    for library in libraries(tx)? {
        let ids: Vec<i64> = missing
            .query_map(params![library, now - WINDOW_MS, TRACK_CHUNK - purged, WINDOW_MS], |r| {
                r.get(0)
            })?
            .collect::<Result<_>>()?;
        let mut touched = Touched::default();
        for &id in &ids {
            touched.track(tx, library, id)?;
            delete.execute([id])?;
            feed::record(tx, library, Entity::Track, id, Op::Delete, now)?;
        }
        catalog::recompute(tx, library, &touched, now)?;
        purged += ids.len();
        if purged == TRACK_CHUNK {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Deletes the images among the next [`CHUNK`] after `after` that nothing uses.
pub(super) fn images(tx: &Transaction<'_>, _: i64, after: &mut i64) -> Result<bool> {
    unused(tx, "images", IMAGE_USED, after)
}

/// Deletes the biographies among the next [`CHUNK`] after `after` that no track uses.
pub(super) fn biographies(tx: &Transaction<'_>, _: i64, after: &mut i64) -> Result<bool> {
    unused(tx, "biographies", BIOGRAPHY_USED, after)
}

/// Looks at the next [`CHUNK`] rows of `table` by ID after `after`, deletes those of which
/// `used` is false (it names the row `r`), moves `after` to the last, and returns whether there
/// were fewer, so the table is done. Each call reads only its own rows, so a large table costs a
/// step at a time.
fn unused(tx: &Transaction<'_>, table: &str, used: &str, after: &mut i64) -> Result<bool> {
    let (last, seen): (Option<i64>, usize) = tx
        .prepare_cached(&format!(
            "SELECT max(id), count(*) FROM (SELECT id FROM {table} WHERE id > ?1 \
             ORDER BY id LIMIT ?2)"
        ))?
        .query_row(params![*after, CHUNK], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let Some(last) = last else { return Ok(true) };
    tx.prepare_cached(&format!(
        "DELETE FROM {table} AS r WHERE id > ?1 AND id <= ?2 AND NOT ({used})"
    ))?
    .execute(params![*after, last])?;
    *after = last;
    Ok(seen < CHUNK)
}
