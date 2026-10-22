//! What a scanned file's art and sidecars resolved to, as the rows its track points at
//! (`design/database.md` §3). The albums and artists take theirs from their tracks as a batch
//! recomputes them ([`crate::db::catalog`]).

use std::path::Path;

use jewelcase_scanner::{ArtworkSource, Biography, ImageInfo, TrackRecord};
use rusqlite::{OptionalExtension, Result, Transaction, params};

use super::entities::upsert_image;
use super::roots::relative;
use crate::db::new_id;

/// A track's art, artist image and biography, by row.
pub struct SidecarRows {
    /// The track's art: its embedded picture, or the `cover.*` it resolved.
    pub image_id: Option<i64>,
    /// Whether that art is embedded, which comes before any `cover.*`.
    pub image_embedded: bool,
    pub artist_image_id: Option<i64>,
    pub biography_id: Option<i64>,
}

/// The rows of `lib` holding `rec`'s art, artist image and biography, added if new. `root_id` is
/// the id of `rec`'s root.
pub fn sidecar_rows(
    tx: &Transaction<'_>,
    lib: i64,
    root_id: i64,
    rec: &TrackRecord,
) -> Result<SidecarRows> {
    let image = |path: &Path, info: &ImageInfo| {
        upsert_image(tx, lib, root_id, &relative(&rec.root, path), info)
    };
    let image_id = rec
        .artwork
        .as_ref()
        .map(|art| match &art.source {
            ArtworkSource::Embedded => image(&rec.path, &art.info),
            ArtworkSource::Sidecar(path) => image(path, &art.info),
        })
        .transpose()?;
    let artist_image_id =
        rec.artist_image.as_ref().map(|img| image(&img.path, &img.info)).transpose()?;
    let biography_id = rec
        .artist_biography
        .as_ref()
        .map(|biography| find_or_create_biography(tx, lib, biography))
        .transpose()?;
    let image_embedded = rec.artwork.as_ref().is_some_and(|a| a.source == ArtworkSource::Embedded);
    Ok(SidecarRows { image_id, image_embedded, artist_image_id, biography_id })
}

/// The row of `lib` holding `biography`, found by its hash, added if new.
fn find_or_create_biography(tx: &Transaction<'_>, lib: i64, biography: &Biography) -> Result<i64> {
    let hash = &biography.hash[..];
    if let Some(id) = tx
        .prepare_cached("SELECT id FROM biographies WHERE library_id = ?1 AND hash = ?2")?
        .query_row(params![lib, hash], |r| r.get(0))
        .optional()?
    {
        return Ok(id);
    }
    let id = new_id()?;
    tx.prepare_cached(
        "INSERT INTO biographies (id, library_id, hash, text) VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![id, lib, hash, &*biography.text])?;
    Ok(id)
}
