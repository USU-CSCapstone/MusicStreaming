//! The library a plugin run reads (`jewelcase_plugins::Library`): its catalog in pages by ID,
//! its roots, which the host confines file access to, and the plugin's own state for it.

use std::path::PathBuf;
use std::sync::Arc;

use jewelcase_plugins::{Album, Artist, Library, Track};
use rusqlite::types::FromSqlError;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::db::plugin_view::{ALBUM, ARTIST, TRACK};
use crate::db::{Database, DbError, Json, by_ids, libraries};

/// One library as a plugin run sees it, for one plugin.
pub struct RunLibrary {
    db: Arc<Database>,
    plugin: Arc<str>,
    library: i64,
}

impl RunLibrary {
    /// `library` in `db`, as `plugin` sees it.
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

/// Logs `error` and answers what the plugin is told instead.
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
    // IDs are positive, so 0 is before every one.
    conn.prepare_cached(&format!("{select} ORDER BY 1 LIMIT ?3"))?
        .query_map(params![library, after.unwrap_or(0), limit], map)?
        .collect()
}

/// A track from a row that starts with [`TRACK`]'s columns.
pub fn track(row: &Row) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        title: row.get(1)?,
        artists: row.get::<_, Json<_>>(2)?.0,
        album_id: row.get(3)?,
        album: row.get(4)?,
        disc_number: row.get(5)?,
        track_number: row.get(6)?,
        release_date: row.get(7)?,
        isrc: row.get(8)?,
        duration_ms: row.get::<_, u64>(9)? / 1000,
        has_lyrics: row.get(10)?,
        root: row.get(11)?,
        path: row.get(12)?,
    })
}

/// An album from a row of [`ALBUM`]'s columns.
fn album(row: &Row) -> rusqlite::Result<Album> {
    Ok(Album {
        id: row.get(0)?,
        title: row.get(1)?,
        artists: row.get::<_, Json<_>>(2)?.0,
        release_date: row.get(3)?,
        has_artwork: row.get(4)?,
    })
}

/// An artist from a row of [`ARTIST`]'s columns.
fn artist(row: &Row) -> rusqlite::Result<Artist> {
    Ok(Artist {
        id: row.get(0)?,
        name: row.get(1)?,
        has_image: row.get(2)?,
        has_biography: row.get(3)?,
    })
}

impl Library for RunLibrary {
    /// A page of the library's tracks, leaving out those missing since a scan.
    async fn tracks(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Track>, String> {
        let select = format!(
            "SELECT {} FROM tracks t \
             WHERE t.library_id = ?1 AND t.id > ?2 AND t.missing_since IS NULL",
            TRACK.columns.join(", ")
        );
        self.read(move |conn, library| page(conn, &select, library, after, limit, track)).await
    }

    /// The tracks with `ids` in this library, leaving out those missing since a scan.
    async fn get_tracks(&mut self, ids: Vec<u64>) -> Result<Vec<Track>, String> {
        self.read(move |conn, library| {
            by_ids(conn, TRACK.columns, TRACK.table, TRACK.rest, library, &ids, track)
        })
        .await
    }

    /// A page of the library's albums that have tracks.
    async fn albums(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Album>, String> {
        let select = format!(
            "SELECT {} FROM albums al \
             WHERE al.library_id = ?1 AND al.id > ?2 AND al.track_count > 0",
            ALBUM.columns.join(", ")
        );
        self.read(move |conn, library| page(conn, &select, library, after, limit, album)).await
    }

    /// A page of the library's artists that have albums or tracks.
    async fn artists(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Artist>, String> {
        let select = format!(
            "SELECT {} FROM artists ar WHERE ar.library_id = ?1 AND ar.id > ?2 \
             AND (ar.album_count > 0 OR ar.track_count > 0)",
            ARTIST.columns.join(", ")
        );
        self.read(move |conn, library| page(conn, &select, library, after, limit, artist)).await
    }

    /// The albums with `ids` in this library that have tracks.
    async fn get_albums(&mut self, ids: Vec<u64>) -> Result<Vec<Album>, String> {
        self.read(move |conn, library| {
            by_ids(conn, ALBUM.columns, ALBUM.table, ALBUM.rest, library, &ids, album)
        })
        .await
    }

    /// The artists with `ids` in this library that have albums or tracks.
    async fn get_artists(&mut self, ids: Vec<u64>) -> Result<Vec<Artist>, String> {
        self.read(move |conn, library| {
            by_ids(conn, ARTIST.columns, ARTIST.table, ARTIST.rest, library, &ids, artist)
        })
        .await
    }

    /// `album`'s tracks in this library, by disc and number, leaving out those missing since a
    /// scan.
    async fn album_tracks(&mut self, album: u64) -> Result<Vec<Track>, String> {
        let select = format!(
            "SELECT {} FROM tracks t \
             WHERE t.library_id = ?1 AND t.album_id = ?2 AND t.missing_since IS NULL \
             ORDER BY t.disc_number, t.track_number IS NULL, t.track_number, t.id",
            TRACK.columns.join(", ")
        );
        self.read(move |conn, library| {
            conn.prepare_cached(&select)?.query_map(params![library, album], track)?.collect()
        })
        .await
    }

    /// The albums with tracks that `artist` is an album artist of, oldest first, undated last.
    async fn artist_albums(&mut self, artist: u64) -> Result<Vec<Album>, String> {
        // Album artists only: an artist's discography, not their appearances
        // (`requirements/artists.md` §2).
        let select = format!(
            "SELECT {} FROM albums al WHERE al.library_id = ?1 AND al.track_count > 0 \
             AND al.id IN (SELECT album_id FROM album_artists WHERE artist_id = ?2) \
             ORDER BY al.release_date IS NULL, al.release_date, al.id",
            ALBUM.columns.join(", ")
        );
        self.read(move |conn, library| {
            conn.prepare_cached(&select)?.query_map(params![library, artist], album)?.collect()
        })
        .await
    }

    /// The library's roots, by ID and path.
    async fn roots(&mut self) -> Result<Vec<(u64, PathBuf)>, String> {
        self.read(|conn, library| {
            let roots = libraries::roots(conn, library)?;
            roots
                .into_iter()
                .map(|root| {
                    let id =
                        u64::try_from(root.id).map_err(|_| FromSqlError::OutOfRange(root.id))?;
                    Ok((id, root.path))
                })
                .collect()
        })
        .await
    }

    /// The value the plugin keeps under `key` for this library.
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

    /// Keeps `value` under `key` for this library, replacing any before it.
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

    /// Forgets the value under `key` for this library.
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
