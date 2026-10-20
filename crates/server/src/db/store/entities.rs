//! The artists, albums, tags, and images a track points at, found by their identity keys or
//! created (`design/database.md` §3).

use jewelcase_core::{name_key, sort_key, tag_key};
use jewelcase_scanner::ImageInfo;
use rusqlite::{OptionalExtension, Result, Transaction, params};

// Each find-or-create reads first, since the key is almost always there already, and inserts
// only when it is not.

/// The artist of `lib` whose name key `name` folds to, added at `now` if new. `None` is the
/// unknown artist.
pub fn find_or_create_artist(
    tx: &Transaction<'_>,
    lib: i64,
    name: Option<&str>,
    now: i64,
) -> Result<i64> {
    let key = name.map(name_key).unwrap_or_default();
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM artists WHERE library_id = ?1 AND name_key = ?2",
            params![lib, key],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    let sort_key = sort_key(name.unwrap_or(""), None);
    tx.query_row(
        "INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at) \
         VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, ?5) RETURNING id",
        params![lib, name.map(str::trim), key, sort_key.as_bytes(), now],
        |r| r.get(0),
    )
}

/// The album of `lib` with `title`'s key and `artists_key`, added at `now` if new. `None` is the
/// unknown album.
pub fn find_or_create_album(
    tx: &Transaction<'_>,
    lib: i64,
    title: Option<&str>,
    artists_key: &str,
    sort_key: &str,
    artist_sort_key: &str,
    now: i64,
) -> Result<i64> {
    let title_key = title.map(name_key).unwrap_or_default();
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM albums WHERE library_id = ?1 AND title_key = ?2 AND artists_key = ?3",
            params![lib, title_key, artists_key],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    tx.query_row(
        "INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key, artist_sort_key, added_at) \
         VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING id",
        params![lib, title.map(str::trim), title_key, artists_key, sort_key.as_bytes(), artist_sort_key.as_bytes(), now],
        |r| r.get(0),
    )
}

/// The genre of `lib` whose tag key `name` folds to, added if new.
pub fn find_or_create_tag(tx: &Transaction<'_>, lib: i64, name: &str) -> Result<i64> {
    let key = tag_key(name);
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM tags WHERE library_id = ?1 AND type = 'genre' AND name_key = ?2",
            params![lib, key],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    let sort_key = sort_key(name, None);
    tx.query_row(
        "INSERT INTO tags (id, library_id, type, name, name_key, sort_key) \
         VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, 'genre', ?2, ?3, ?4) RETURNING id",
        params![lib, name.trim(), key, sort_key.as_bytes()],
        |r| r.get(0),
    )
}

/// The image of `lib` with `info`'s hash. A new one is added at `rel` under `root_id`; an
/// existing one keeps its path.
pub fn upsert_image(
    tx: &Transaction<'_>,
    lib: i64,
    root_id: i64,
    rel: &str,
    info: &ImageInfo,
) -> Result<i64> {
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM images WHERE library_id = ?1 AND hash = ?2",
            params![lib, info.hash],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    tx.query_row(
        "INSERT INTO images (id, library_id, hash, width, height, placeholder, root_id, path) \
         VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, NULL, ?5, ?6) RETURNING id",
        params![lib, info.hash, info.width as i64, info.height as i64, root_id, rel],
        |r| r.get(0),
    )
}
