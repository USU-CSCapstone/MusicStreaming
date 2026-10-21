//! The background jobs' queues and results: loudness and waveform analysis, and image
//! placeholders. Each queue is the rows still without a result.

use jewelcase_scanner::{AnalysisResult, FileToDecode, PickedTrack};
use rusqlite::{OptionalExtension, params};

use super::SqliteStore;
use super::roots::{Roots, join};
use crate::db::catalog;
use crate::db::feed::{self, Entity, Op};
use crate::db::{DbError, now_ms};

/// The analysis queue: library `?1`'s tracks analyzed below version `?2`, at most `?3` of them,
/// in the order of `tracks_unanalyzed`, so each pick reads the head of one index range.
const UNANALYZED: &str = "SELECT id, file_size, file_mtime, root_id, path FROM tracks \
     WHERE library_id = ?1 AND missing_since IS NULL AND file_mtime IS NOT NULL \
     AND analyzer_version < ?2 \
     ORDER BY analyzer_version, added_at DESC, id LIMIT ?3";

/// Tracks of `lib` analyzed below `analyzer_version`, each with its file and the size and mtime
/// that file had when picked: those never analyzed (version 0) before those an older analyzer
/// did, and newest first within each.
pub fn next_unanalyzed(
    store: &SqliteStore,
    lib: i64,
    analyzer_version: u32,
    limit: usize,
) -> Result<Vec<(PickedTrack, FileToDecode)>, DbError> {
    let found = store.db.read_blocking(move |conn| {
        conn.prepare_cached(UNANALYZED)?
            .query_map(params![lib, analyzer_version as i64, limit as i64], |r| {
                let (id, size, mtime_ms) =
                    (r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?);
                let track =
                    PickedTrack { id: id as u64, size: size as u64, mtime_ms: mtime_ms as u64 };
                Ok((track, r.get(3)?, r.get(4)?))
            })?
            .collect()
    })?;
    Ok(files(&store.roots, lib, found))
}

/// Saves `result`'s loudness and waveform on the track, refreshes its album's loudness, and
/// records both in the feed. Does nothing when the track is not in `lib`, or when its file's size
/// or mtime differs from those it was picked with: a scan found the file changed during the
/// decode, so the result may have measured other audio.
pub fn store_analysis(
    store: &SqliteStore,
    lib: i64,
    track: PickedTrack,
    result: AnalysisResult,
) -> Result<(), DbError> {
    store.db.write_blocking(move |tx| {
        let now = now_ms();
        let id = track.id as i64;
        // No row means the track is not in this library or its file changed, so nothing else
        // may change.
        let Some(album_id) = tx
            .query_row(
                "UPDATE tracks SET loudness_lufs = ?2, peak_dbtp = ?3, analyzer_version = ?4 \
                 WHERE id = ?1 AND library_id = ?5 AND file_size = ?6 AND file_mtime = ?7 \
                 RETURNING album_id",
                params![
                    id,
                    result.integrated_lufs,
                    result.true_peak_dbtp,
                    result.analyzer_version as i64,
                    lib,
                    track.size as i64,
                    track.mtime_ms as i64
                ],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        else {
            return Ok(());
        };
        if result.waveform.peaks.is_empty() {
            tx.execute("DELETE FROM track_waveforms WHERE track_id = ?1", [id])?;
        } else {
            tx.execute(
                "INSERT OR REPLACE INTO track_waveforms (library_id, track_id, data) VALUES (?1, ?2, ?3)",
                params![lib, id, result.waveform.to_blob()],
            )?;
        }
        catalog::recompute_album_loudness(tx, album_id)?;
        feed::record(tx, lib, Entity::Album, album_id, Op::Upsert, now)?;
        feed::record(tx, lib, Entity::Track, id, Op::Upsert, now)
    })
}

/// Images of `lib` without a placeholder, by content hash, each with its file.
pub fn next_without_placeholder(
    store: &SqliteStore,
    lib: i64,
    limit: usize,
) -> Result<Vec<(Vec<u8>, FileToDecode)>, DbError> {
    let found = store.db.read_blocking(move |conn| {
        conn.prepare_cached(
            "SELECT hash, root_id, path FROM images \
             WHERE library_id = ?1 AND placeholder IS NULL LIMIT ?2",
        )?
        .query_map(params![lib, limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect()
    })?;
    Ok(files(&store.roots, lib, found))
}

/// Each of `found`, a key with a root ID and a path relative to it, with its file. A row whose
/// root is no longer active is left out.
fn files<K>(roots: &Roots, lib: i64, found: Vec<(K, i64, String)>) -> Vec<(K, FileToDecode)> {
    found
        .into_iter()
        .filter_map(|(key, root_id, rel)| {
            let root = roots.root(lib, root_id)?;
            let path = join(&root, &rel);
            Some((key, FileToDecode { root, path }))
        })
        .collect()
}

pub fn store_placeholder(
    store: &SqliteStore,
    lib: i64,
    hash: &[u8],
    placeholder: Vec<u8>,
) -> Result<(), DbError> {
    let hash = hash.to_vec();
    store.db.write_blocking(move |tx| {
        let now = now_ms();
        let changed = !placeholder.is_empty();
        let Some(image) = tx
            .query_row(
                "UPDATE images SET placeholder = ?3 WHERE library_id = ?1 AND hash = ?2 RETURNING id",
                params![lib, hash, placeholder],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        else {
            return Ok(());
        };
        // What clients see of an album or artist includes its image's placeholder. One
        // that could not be made leaves them as they were.
        if changed {
            for (entity, table) in [(Entity::Album, "albums"), (Entity::Artist, "artists")] {
                let ids: Vec<i64> = tx
                    .prepare_cached(&format!(
                        "SELECT id FROM {table} WHERE library_id = ?1 AND image_id = ?2"
                    ))?
                    .query_map([lib, image], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                for id in ids {
                    feed::record(tx, lib, entity, id, Op::Upsert, now)?;
                }
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests;
