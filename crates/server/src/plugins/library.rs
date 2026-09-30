//! The library a plugin run reads (`jewelcase_plugins::Library`): its tracks in pages, and
//! where a track's audio is, which only the host ever sees.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use jewelcase_plugins::{Library, Track};
use rusqlite::{OptionalExtension, Row, params};

use crate::db::Database;

/// One library's tracks, read in `id` order. A plugin pages by offset, and SQLite skips an
/// offset row by row, so reading onward from where the last page ended keeps a whole pass
/// linear: each page starts after the last ID it returned.
pub struct RunLibrary {
    db: Arc<Database>,
    library: i64,
    /// The offset the next page would start at, and the last ID before it.
    next: Option<(u32, i64)>,
}

impl RunLibrary {
    pub fn new(db: Arc<Database>, library: i64) -> RunLibrary {
        RunLibrary { db, library, next: None }
    }
}

const TRACK: &str = "SELECT t.id, t.title, \
     (SELECT json_group_array(artist_name) FROM (SELECT artist_name FROM track_artists \
      WHERE track_id = t.id AND artist_name IS NOT NULL ORDER BY position)), \
     al.title, t.duration_us, t.lyrics_kind <> 'none' \
     FROM tracks t JOIN albums al ON al.id = t.album_id \
     WHERE t.library_id = ?1 AND t.missing_since IS NULL";

fn track(row: &Row) -> rusqlite::Result<Track> {
    let artists: String = row.get(2)?;
    Ok(Track {
        id: row.get::<_, i64>(0)? as u64,
        title: row.get(1)?,
        artists: serde_json::from_str(&artists).unwrap_or_default(),
        album: row.get(3)?,
        duration_ms: (row.get::<_, i64>(4)? / 1000) as u64,
        has_lyrics: row.get(5)?,
    })
}

impl Library for RunLibrary {
    async fn tracks(&mut self, offset: u32, limit: u32) -> Result<Vec<Track>, String> {
        let library = self.library;
        let after = self.next.filter(|(at, _)| *at == offset).map(|(_, id)| id);
        let page = self
            .db
            .read(move |conn| match after {
                Some(after) => conn
                    .prepare_cached(&format!("{TRACK} AND t.id > ?2 ORDER BY t.id LIMIT ?3"))?
                    .query_map(params![library, after, limit], track)?
                    .collect::<rusqlite::Result<Vec<_>>>(),
                None => conn
                    .prepare_cached(&format!("{TRACK} ORDER BY t.id LIMIT ?3 OFFSET ?2"))?
                    .query_map(params![library, offset, limit], track)?
                    .collect(),
            })
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "cannot read tracks for a plugin");
                "the library could not be read".to_owned()
            })?;
        if let Some(last) = page.last() {
            self.next = Some((offset + page.len() as u32, last.id as i64));
        }
        Ok(page)
    }

    async fn audio_without_lyrics(&mut self, track: u64) -> Result<PathBuf, String> {
        let library = self.library;
        let found = self
            .db
            .read(move |conn| {
                conn.prepare_cached(
                    "SELECT r.path, t.path, t.lyrics_kind <> 'none' FROM tracks t \
                     JOIN library_roots r ON r.id = t.root_id \
                     WHERE t.id = ?1 AND t.library_id = ?2 AND t.missing_since IS NULL",
                )?
                .query_row(params![track as i64, library], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, bool>(2)?))
                })
                .optional()
            })
            .await
            .map_err(|_| "the library could not be read".to_owned())?;
        match found {
            None => Err("no such track in this library".into()),
            Some((_, _, true)) => Err("this track already has lyrics".into()),
            Some((root, path, false)) => Ok(Path::new(&root).join(path)),
        }
    }
}
