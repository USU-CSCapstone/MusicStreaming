//! The library a plugin run reads (`jewelcase_plugins::Library`): its catalog in pages by ID,
//! its roots, which the host confines file access to, and the plugin's own state for it.

use std::path::PathBuf;
use std::sync::Arc;

use jewelcase_plugins::{Album, Artist, Library, Track};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::db::{Database, DbError};

pub struct RunLibrary {
    db: Arc<Database>,
    plugin: Arc<str>,
    library: i64,
}

impl RunLibrary {
    pub fn new(db: Arc<Database>, plugin: &str, library: i64) -> RunLibrary {
        RunLibrary { db, plugin: plugin.into(), library }
    }

    /// Runs `f` on a reader with this library's ID. A failure is logged here and reaches the
    /// plugin only as "could not be read", which says nothing of the database.
    async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection, i64) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T, String> {
        let library = self.library;
        self.db.read(move |conn| f(conn, library)).await.map_err(failed)
    }
}

fn failed(error: DbError) -> String {
    tracing::error!(%error, "cannot serve a plugin's library call");
    "the library could not be read".to_owned()
}

/// Up to `limit` rows of `select` with IDs after `after`, in ID order. `select` names this
/// library `?1` and the ID it continues from `?2`, and has the ID first.
fn page<T>(
    conn: &Connection,
    select: &str,
    library: i64,
    after: Option<u64>,
    limit: u32,
    map: fn(&Row) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>> {
    let after = after.map_or(-1, |id| id as i64);
    conn.prepare_cached(&format!("{select} ORDER BY 1 LIMIT ?3"))?
        .query_map(params![library, after, limit], map)?
        .collect()
}

pub const TRACK_COLUMNS: &str = "SELECT t.id, t.title, \
     (SELECT json_group_array(artist_name) FROM (SELECT artist_name FROM track_artists \
      WHERE track_id = t.id AND artist_name IS NOT NULL ORDER BY position)), \
     t.album_id, al.title, t.disc_number, t.track_number, t.release_date, t.isrc, t.duration_us, \
     t.lyrics_kind <> 'none', t.root_id, t.path";

pub fn track(row: &Row) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get::<_, i64>(0)? as u64,
        title: row.get(1)?,
        artists: names(row, 2)?,
        album_id: row.get::<_, i64>(3)? as u64,
        album: row.get(4)?,
        disc_number: row.get(5)?,
        track_number: row.get(6)?,
        release_date: row.get(7)?,
        isrc: row.get(8)?,
        duration_ms: (row.get::<_, i64>(9)? / 1000) as u64,
        has_lyrics: row.get(10)?,
        root: row.get::<_, i64>(11)? as u64,
        path: row.get(12)?,
    })
}

/// An album's columns, as `al`. Only one with tracks is shown, as the browse lists show them.
const ALBUM_COLUMNS: &str = "SELECT al.id, al.title, \
     (SELECT json_group_array(name) FROM (SELECT ar.name FROM album_artists aa \
      JOIN artists ar ON ar.id = aa.artist_id WHERE aa.album_id = al.id AND ar.name IS NOT NULL \
      ORDER BY aa.position)), \
     al.release_date, al.track_count, al.image_id IS NOT NULL";

fn album(row: &Row) -> rusqlite::Result<Album> {
    Ok(Album {
        id: row.get::<_, i64>(0)? as u64,
        title: row.get(1)?,
        artists: names(row, 2)?,
        release_date: row.get(3)?,
        track_count: row.get(4)?,
        has_artwork: row.get(5)?,
    })
}

/// An artist's columns, as `ar`. Only one with music is shown.
const ARTIST_COLUMNS: &str = "SELECT ar.id, ar.name, ar.album_count, ar.track_count, \
     ar.image_id IS NOT NULL, coalesce(ar.biography, '') <> ''";

fn artist(row: &Row) -> rusqlite::Result<Artist> {
    Ok(Artist {
        id: row.get::<_, i64>(0)? as u64,
        name: row.get(1)?,
        album_count: row.get(2)?,
        track_count: row.get(3)?,
        has_image: row.get(4)?,
        has_biography: row.get(5)?,
    })
}

/// `select`'s rows for these IDs, in their order, leaving out any not in `library`. `select`
/// names the IDs, as JSON, `?2`, and this library `?1`.
fn by_ids<T>(
    conn: &Connection,
    select: &str,
    library: i64,
    ids: &[u64],
    map: fn(&Row) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>> {
    let ids = serde_json::to_string(ids).expect("integers serialize");
    conn.prepare_cached(select)?.query_map(params![library, ids], map)?.collect()
}

/// A JSON array of names in column `index`.
fn names(row: &Row, index: usize) -> rusqlite::Result<Vec<String>> {
    let json: String = row.get(index)?;
    serde_json::from_str(&json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}

impl Library for RunLibrary {
    async fn tracks(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Track>, String> {
        let select = format!(
            "{TRACK_COLUMNS} FROM tracks t JOIN albums al ON al.id = t.album_id \
             WHERE t.library_id = ?1 AND t.id > ?2 AND t.missing_since IS NULL"
        );
        self.read(move |conn, library| page(conn, &select, library, after, limit, track)).await
    }

    async fn get_tracks(&mut self, ids: Vec<u64>) -> Result<Vec<Track>, String> {
        let ids = serde_json::to_string(&ids).expect("integers serialize");
        self.read(move |conn, library| {
            // The IDs lead the join, so the tracks come out in the order asked for.
            conn.prepare_cached(&format!(
                "{TRACK_COLUMNS} FROM json_each(?2) ids CROSS JOIN tracks t ON t.id = ids.value \
                 JOIN albums al ON al.id = t.album_id \
                 WHERE t.library_id = ?1 AND t.missing_since IS NULL ORDER BY ids.key"
            ))?
            .query_map(params![library, ids], track)?
            .collect()
        })
        .await
    }

    async fn albums(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Album>, String> {
        let select = format!(
            "{ALBUM_COLUMNS} FROM albums al \
             WHERE al.library_id = ?1 AND al.id > ?2 AND al.track_count > 0"
        );
        self.read(move |conn, library| page(conn, &select, library, after, limit, album)).await
    }

    async fn artists(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Artist>, String> {
        let select = format!(
            "{ARTIST_COLUMNS} FROM artists ar WHERE ar.library_id = ?1 AND ar.id > ?2 \
             AND (ar.album_count > 0 OR ar.track_count > 0)"
        );
        self.read(move |conn, library| page(conn, &select, library, after, limit, artist)).await
    }

    async fn get_albums(&mut self, ids: Vec<u64>) -> Result<Vec<Album>, String> {
        // The IDs lead the join, so the albums come out in the order asked for.
        let select = format!(
            "{ALBUM_COLUMNS} FROM json_each(?2) ids CROSS JOIN albums al ON al.id = ids.value \
             WHERE al.library_id = ?1 AND al.track_count > 0 ORDER BY ids.key"
        );
        self.read(move |conn, library| by_ids(conn, &select, library, &ids, album)).await
    }

    async fn get_artists(&mut self, ids: Vec<u64>) -> Result<Vec<Artist>, String> {
        let select = format!(
            "{ARTIST_COLUMNS} FROM json_each(?2) ids CROSS JOIN artists ar ON ar.id = ids.value \
             WHERE ar.library_id = ?1 AND (ar.album_count > 0 OR ar.track_count > 0) \
             ORDER BY ids.key"
        );
        self.read(move |conn, library| by_ids(conn, &select, library, &ids, artist)).await
    }

    async fn album_tracks(&mut self, album: u64) -> Result<Vec<Track>, String> {
        let select = format!(
            "{TRACK_COLUMNS} FROM tracks t JOIN albums al ON al.id = t.album_id \
             WHERE t.library_id = ?1 AND t.album_id = ?2 AND t.missing_since IS NULL \
             ORDER BY t.disc_number, t.track_number IS NULL, t.track_number, t.id"
        );
        self.read(move |conn, library| {
            conn.prepare_cached(&select)?
                .query_map(params![library, album as i64], track)?
                .collect()
        })
        .await
    }

    async fn artist_albums(&mut self, artist: u64) -> Result<Vec<Album>, String> {
        // Album artists only: an artist's discography, not their appearances
        // (`requirements/artists.md` §2).
        let select = format!(
            "{ALBUM_COLUMNS} FROM albums al WHERE al.library_id = ?1 AND al.track_count > 0 \
             AND al.id IN (SELECT album_id FROM album_artists WHERE artist_id = ?2) \
             ORDER BY al.release_date IS NULL, al.release_date, al.id"
        );
        self.read(move |conn, library| {
            conn.prepare_cached(&select)?
                .query_map(params![library, artist as i64], album)?
                .collect()
        })
        .await
    }

    async fn roots(&mut self) -> Result<Vec<(u64, PathBuf)>, String> {
        self.read(|conn, library| {
            conn.prepare_cached(
                "SELECT id, path FROM library_roots WHERE library_id = ?1 AND removed_at IS NULL",
            )?
            .query_map([library], |row| {
                Ok((row.get::<_, i64>(0)? as u64, PathBuf::from(row.get::<_, String>(1)?)))
            })?
            .collect()
        })
        .await
    }

    async fn state_get(&mut self, key: String) -> Result<Option<Vec<u8>>, String> {
        let plugin = self.plugin.clone();
        self.read(move |conn, library| {
            conn.prepare_cached(
                "SELECT value FROM plugin_state WHERE plugin_id = ?1 AND library_id = ?2 AND key = ?3",
            )?
            .query_row(params![plugin, library, key], |row| row.get(0))
            .optional()
        })
        .await
    }

    async fn state_set(&mut self, key: String, value: Vec<u8>) -> Result<(), String> {
        let (plugin, library) = (self.plugin.clone(), self.library);
        self.db
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO plugin_state (plugin_id, library_id, key, value) \
                     VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT (plugin_id, library_id, key) DO UPDATE SET value = excluded.value",
                    params![plugin, library, key, value],
                )
            })
            .await
            .map(drop)
            .map_err(failed)
    }

    async fn state_delete(&mut self, key: String) -> Result<(), String> {
        let (plugin, library) = (self.plugin.clone(), self.library);
        self.db
            .write(move |tx| {
                tx.execute(
                    "DELETE FROM plugin_state WHERE plugin_id = ?1 AND library_id = ?2 AND key = ?3",
                    params![plugin, library, key],
                )
            })
            .await
            .map(drop)
            .map_err(failed)
    }
}

#[cfg(test)]
mod tests;
