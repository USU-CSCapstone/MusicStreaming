//! Catalog rows for tests: artists, albums, tracks, and images.

use jewelcase_core::sort_key;
use rusqlite::{Connection, Result, params};

/// An artist, added at `added_at`.
pub struct Artist {
    pub id: i64,
    pub library: i64,
    pub name: Option<String>,
    pub sort_key: Vec<u8>,
    pub added_at: i64,
}

impl Artist {
    /// Artist `id` of `library` called `name`, sorted as the scanner would sort it. Its counts
    /// follow the credits a test adds.
    pub fn new(id: i64, library: i64, name: &str) -> Artist {
        let sort_key = sort_key(name, None).into_bytes();
        let name = Some(name.to_owned());
        Artist { id, library, name, sort_key, added_at: 0 }
    }

    /// Adds the artist, with its ID as its name key so no two collide.
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        conn.prepare_cached(
            "INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at) \
             VALUES (?1, ?2, ?3, ?1, ?4, ?5)",
        )?
        .execute(params![self.id, self.library, self.name, self.sort_key, self.added_at])
        .map(drop)
    }
}

/// Credits `artist` first among album `album`'s artists, in `library`.
pub fn album_artist(conn: &Connection, library: i64, album: i64, artist: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO album_artists (library_id, album_id, position, artist_id) \
         VALUES (?1, ?2, 0, ?3)",
        [library, album, artist],
    )
    .map(drop)
}

/// Credits `artist` at `position` among track `track`'s artists, in `library`, written in the
/// file as `name`.
pub fn track_artist(
    conn: &Connection,
    library: i64,
    track: i64,
    position: i64,
    artist: i64,
    name: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO track_artists (library_id, track_id, position, artist_id, artist_name) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![library, track, position, artist, name],
    )
    .map(drop)
}

/// An album with no artists, added at `added_at`.
pub struct Album {
    pub id: i64,
    pub library: i64,
    pub title: Option<String>,
    pub sort_key: Vec<u8>,
    pub artist_sort_key: Vec<u8>,
    pub release_date: Option<String>,
    pub track_count: i64,
    pub available_track_count: i64,
    pub disc_count: i64,
    pub duration_us: i64,
    pub loudness_lufs: Option<f64>,
    pub peak_dbtp: Option<f64>,
    pub added_at: i64,
}

impl Album {
    /// Album `id` of `library` with no title, date, or tracks counted, and keys that sort it
    /// last.
    pub fn new(id: i64, library: i64) -> Album {
        Album {
            id,
            library,
            title: None,
            sort_key: Vec::new(),
            artist_sort_key: Vec::new(),
            release_date: None,
            track_count: 0,
            available_track_count: 0,
            disc_count: 0,
            duration_us: 0,
            loudness_lufs: None,
            peak_dbtp: None,
            added_at: 0,
        }
    }

    /// Adds the album, with its ID as its title key so no two collide.
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        conn.prepare_cached(
            "INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key, \
             artist_sort_key, release_date, track_count, available_track_count, disc_count, \
             duration_us, loudness_lufs, peak_dbtp, added_at) \
             VALUES (?1, ?2, ?3, ?1, '', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )?
        .execute(params![
            self.id,
            self.library,
            self.title,
            self.sort_key,
            self.artist_sort_key,
            self.release_date,
            self.track_count,
            self.available_track_count,
            self.disc_count,
            self.duration_us,
            self.loudness_lufs,
            self.peak_dbtp,
            self.added_at
        ])
        .map(drop)
    }
}

/// A track on its library's root, in a file last modified at the epoch.
pub struct Track {
    pub id: i64,
    pub library: i64,
    pub album: i64,
    pub path: String,
    pub file_size: i64,
    pub missing_since: Option<i64>,
    pub title: String,
    pub sort_key: Vec<u8>,
    pub artist_sort_key: Vec<u8>,
    pub album_sort_key: Vec<u8>,
    pub disc_number: i64,
    pub track_number: Option<i64>,
    pub release_date: Option<String>,
    pub codec: &'static str,
    pub container: &'static str,
    pub lossless: bool,
    pub bit_depth: Option<i64>,
    pub duration_us: i64,
    pub loudness_lufs: Option<f64>,
    pub peak_dbtp: Option<f64>,
    pub added_at: i64,
}

impl Track {
    /// Track `id` of `library` on `album`: a one-second, 10-byte FLAC at `{id}.flac` called
    /// `Track {id}`, with no artist, disc, or number.
    pub fn new(id: i64, library: i64, album: i64) -> Track {
        let title = format!("Track {id}");
        Track {
            id,
            library,
            album,
            path: format!("{id}.flac"),
            file_size: 10,
            missing_since: None,
            sort_key: sort_key(&title, None).into_bytes(),
            title,
            artist_sort_key: Vec::new(),
            album_sort_key: Vec::new(),
            disc_number: 0,
            track_number: None,
            release_date: None,
            codec: "flac",
            container: "flac",
            lossless: true,
            bit_depth: None,
            duration_us: 1_000_000,
            loudness_lufs: None,
            peak_dbtp: None,
            added_at: 0,
        }
    }

    /// Adds the track, on its library's root.
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        conn.prepare_cached(
            "INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size, file_mtime, \
             missing_since, title, sort_key, artist_sort_key, album_sort_key, disc_number, \
             track_number, release_date, codec, container, lossless, sample_rate_hz, bit_depth, \
             channels, duration_us, loudness_lufs, peak_dbtp, added_at) \
             VALUES (?1, ?2, ?3, (SELECT id FROM library_roots WHERE library_id = ?2), ?4, ?5, 0, \
                     ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 44100, ?17, 2, ?18, ?19, \
                     ?20, ?21)",
        )?
        .execute(params![
            self.id,
            self.library,
            self.album,
            self.path,
            self.file_size,
            self.missing_since,
            self.title,
            self.sort_key,
            self.artist_sort_key,
            self.album_sort_key,
            self.disc_number,
            self.track_number,
            self.release_date,
            self.codec,
            self.container,
            self.lossless,
            self.bit_depth,
            self.duration_us,
            self.loudness_lufs,
            self.peak_dbtp,
            self.added_at
        ])
        .map(drop)
    }
}

/// A PNG image file on its library's root, 1 by 1 pixels.
pub struct Image {
    pub id: i64,
    pub library: i64,
    pub hash: Vec<u8>,
    pub path: &'static str,
    /// Its ThumbHash: `None` until the placeholder job runs, empty if it could not decode it.
    pub placeholder: Option<Vec<u8>>,
}

impl Image {
    /// Image `id` of `library` at `path`, hashed as its ID's bytes, with no placeholder yet.
    pub fn new(id: i64, library: i64, path: &'static str) -> Image {
        let hash = id.to_be_bytes().to_vec();
        Image { id, library, hash, path, placeholder: None }
    }

    /// Adds the image.
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO images (id, library_id, hash, width, height, placeholder, root_id, path) \
             VALUES (?1, ?2, ?3, 1, 1, ?4, (SELECT id FROM library_roots WHERE library_id = ?2), ?5)",
            params![self.id, self.library, self.hash, self.placeholder, self.path],
        )
        .map(drop)
    }
}

/// The sort key `{prefix} {hex}`, which spreads consecutive `i` over the key space as random
/// keys would, for fixtures large enough for the query planner to weigh.
pub fn scattered_key(prefix: &str, i: i64) -> Vec<u8> {
    format!("{prefix} {:08x}", i * 2_654_435_761 % 4_294_967_296).into_bytes()
}
