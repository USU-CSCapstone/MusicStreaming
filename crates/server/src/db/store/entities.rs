//! The artists, albums, tags, and images a track points at, found by their identity keys or
//! created (`design/database.md` §3).

use jewelcase_core::{name_key, sort_key, tag_key};
use jewelcase_scanner::ImageInfo;
use rusqlite::{OptionalExtension, Result, Transaction, params};

use crate::db::new_id;

// Each find-or-create reads first, since the key is almost always there already, and inserts
// only when it is not. An image is upserted instead, since its row takes the path it is found at.

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
    let id = new_id()?;
    tx.execute(
        "INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, lib, name.map(str::trim), key, sort_key.as_bytes(), now],
    )?;
    Ok(id)
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
    let id = new_id()?;
    tx.execute(
        "INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key, artist_sort_key, added_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, lib, title.map(str::trim), title_key, artists_key, sort_key.as_bytes(), artist_sort_key.as_bytes(), now],
    )?;
    Ok(id)
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
    let id = new_id()?;
    tx.execute(
        "INSERT INTO tags (id, library_id, type, name, name_key, sort_key) \
         VALUES (?1, ?2, 'genre', ?3, ?4, ?5)",
        params![id, lib, name.trim(), key, sort_key.as_bytes()],
    )?;
    Ok(id)
}

/// The image of `lib` with `info`'s hash, at `rel` under `root_id`: added if new, and otherwise
/// moved there, so it follows its file to wherever a scan last found it.
pub fn upsert_image(
    tx: &Transaction<'_>,
    lib: i64,
    root_id: i64,
    rel: &str,
    info: &ImageInfo,
) -> Result<i64> {
    tx.query_row(
        "INSERT INTO images (id, library_id, hash, width, height, placeholder, root_id, path) \
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7) \
         ON CONFLICT (library_id, hash) DO UPDATE SET root_id = excluded.root_id, path = excluded.path \
         RETURNING id",
        params![new_id()?, lib, info.hash, info.width as i64, info.height as i64, root_id, rel],
        |r| r.get(0),
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rusqlite::Connection;

    use super::*;
    use crate::db::testing::library;
    use crate::db::{Database, libraries};

    /// The id of library `lib`'s first root.
    fn root(conn: &Connection, lib: i64) -> Result<i64> {
        conn.query_row("SELECT id FROM library_roots WHERE library_id = ?1", [lib], |r| r.get(0))
    }

    /// Each library's images, as `(library, id, root, path)` in id order.
    fn images(conn: &Connection) -> Result<Vec<(i64, i64, i64, String)>> {
        conn.prepare("SELECT library_id, id, root_id, path FROM images ORDER BY library_id, id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect()
    }

    /// An image found again by its hash, as after its folder was renamed or moved to another
    /// root, is the same row at the root and path it was found at; the same bytes in another
    /// library are that library's own row.
    #[test]
    fn an_image_found_by_hash_follows_its_file_within_its_library() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(&temp.path().join("jewelcase.db")).unwrap();
        let info = ImageInfo { hash: vec![7; 32], width: 2, height: 1 };
        let (roots, ids, renamed, moved) = db
            .write_blocking(move |tx| {
                library(tx, 1, "Music", "/music")?;
                library(tx, 2, "Other", "/other")?;
                let (one, two) = (root(tx, 1)?, root(tx, 2)?);
                let more = libraries::add_root(tx, 1, Path::new("/more"))?;
                let first = upsert_image(tx, 1, one, "A/X/cover.png", &info)?;
                let other = upsert_image(tx, 2, two, "A/X/cover.png", &info)?;
                let again = upsert_image(tx, 1, one, "A/Y/cover.png", &info)?;
                let renamed = images(tx)?;
                let last = upsert_image(tx, 1, more, "B/cover.png", &info)?;
                Ok(([one, two, more], [first, other, again, last], renamed, images(tx)?))
            })
            .unwrap();
        let ([one, two, more], [first, other, again, last]) = (roots, ids);
        assert!([first, other, more].iter().all(|&id| id > i64::from(u32::MAX)), "drawn ids");
        assert_eq!([again, last], [first, first], "found by its hash");
        assert_ne!(other, first, "another library's own row");
        let elsewhere = (2, other, two, "A/X/cover.png".to_owned());
        assert_eq!(renamed, [(1, first, one, "A/Y/cover.png".to_owned()), elsewhere.clone()]);
        assert_eq!(moved, [(1, first, more, "B/cover.png".to_owned()), elsewhere]);
    }
}
