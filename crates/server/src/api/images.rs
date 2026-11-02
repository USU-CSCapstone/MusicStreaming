//! Artwork: `getImage` (`api/openapi.yaml`), resized and cached by [`Images`]. A cached copy
//! never goes stale, so the client may cache it forever too.

mod cache;

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use rusqlite::OptionalExtension;
use serde::Deserialize;
use tower_http::services::ServeFile;

pub use cache::Images;

use super::extract::{Path, Query};
use super::scope::Scope;
use super::{Code, Id, Problem, playlists};
use crate::db::{Database, libraries};
use jewelcase_scanner::FileToDecode;

/// The sizes images are resized to, as the longest edge in pixels. A request rounds up to the
/// next one, and no further than the first that holds the whole image, which is never enlarged,
/// so a handful of cached copies serves every display.
const SIZES: [u32; 7] = [64, 128, 256, 512, 1024, 2048, 4096];
const DEFAULT_SIZE: u32 = 512;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageQuery {
    size: Option<u32>,
}

/// What an image ID names: a library image, or the mosaic of four of them.
enum Target {
    Image(i64),
    Mosaic([i64; 4]),
}

/// The ID of the mosaic of `ids`, the first of which is the top left, as the spec's `ImageRef`
/// carries it for a playlist: `m` and the four image IDs, each as [`Id`] spells it, joined by `-`.
pub fn mosaic_id(ids: [i64; 4]) -> String {
    let [a, b, c, d] = ids;
    format!("m{a}-{b}-{c}-{d}")
}

impl Target {
    /// The target `text` names, if it is an [`Id`] or a [`mosaic_id`] of four in its one canonical
    /// spelling. Whether a playlist holds the mosaic is for [`get`] to ask.
    fn parse(text: &str) -> Option<Target> {
        let Some(ids) = text.strip_prefix('m') else {
            return Id::canonical(text).map(|Id(id)| Target::Image(id));
        };
        let ids: Vec<i64> =
            ids.split('-').map(|id| Id::canonical(id).map(|Id(id)| id)).collect::<Option<_>>()?;
        Some(Target::Mosaic(ids.try_into().ok()?))
    }
}

/// An image's content hash, its longest side, and the file it is read from.
type Found = (Vec<u8>, u32, FileToDecode);

/// The content hash, longest side and file of `library`'s image `image`. `404` when there is no
/// such image or its root is no longer in the library.
fn find(conn: &rusqlite::Connection, library: i64, image: i64) -> Result<Found, Problem> {
    let (hash, longest, root, path) = conn
        .prepare_cached(
            "SELECT hash, max(width, height), root_id, path FROM images \
             WHERE library_id = ?1 AND id = ?2",
        )?
        .query_row([library, image], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .optional()?
        .ok_or_else(Problem::not_found)?;
    let file = libraries::file(conn, library, root, &path)?.ok_or_else(Problem::not_found)?;
    Ok((hash, longest, file))
}

/// The spec's `getImage`: the image resized to the first of [`SIZES`] at or above `size`
/// ([`DEFAULT_SIZE`] if not given), or to the first that holds the whole image if that is
/// smaller. An ID that is a [`mosaic_id`] is the four images composed into a square the same way,
/// whose side is no more than twice the longest of theirs. An image under a root no longer in its
/// library, or whose file is missing or reached through a symlink, is `404`, and so is a mosaic of
/// which any image is, or that is not the artwork of one of the caller's own playlists in the
/// library; an ID that is not one of the caller's library's, or is malformed, is too.
pub async fn get(
    State(db): State<Arc<Database>>,
    State(images): State<Arc<Images>>,
    Scope { user, library }: Scope,
    Path((_, image)): Path<(Id, String)>,
    Query(query): Query<ImageQuery>,
    request: Request,
) -> Result<Response, Problem> {
    let size = match query.size.unwrap_or(DEFAULT_SIZE) {
        requested @ 16..=4096 => SIZES.into_iter().find(|&size| size >= requested),
        _ => None,
    }
    .ok_or_else(|| Problem::invalid("size must be from 16 to 4096"))?;
    let target = Target::parse(&image).ok_or_else(Problem::not_found)?;
    let whole = |longest: u32| SIZES.into_iter().find(|&whole| whole >= longest);
    let resized = match target {
        Target::Image(id) => {
            let (hash, longest, source) = db.try_read(move |conn| find(conn, library, id)).await?;
            let size = whole(longest).map_or(size, |whole| size.min(whole));
            images.resized(library, &hash, size, source).await
        }
        Target::Mosaic(ids) => {
            let found = db
                .try_read(move |conn| -> Result<_, Problem> {
                    if !playlists::holds_artwork(conn, user, library, &mosaic_id(ids))? {
                        return Err(Problem::not_found());
                    }
                    let [a, b, c, d] = ids;
                    Ok([
                        find(conn, library, a)?,
                        find(conn, library, b)?,
                        find(conn, library, c)?,
                        find(conn, library, d)?,
                    ])
                })
                .await?;
            let longest = found.iter().map(|(_, longest, _)| *longest).max().unwrap_or(0);
            let size = whole(longest.saturating_mul(2)).map_or(size, |whole| size.min(whole));
            images.mosaic(library, user, ids, size, found.map(|(_, _, source)| source)).await
        }
    }
    .map_err(|error| {
        // The cause stays in the log: it names a path on the host.
        tracing::warn!(%error, image, "cannot resize an image");
        Problem::new(Code::Internal)
    })?;
    let resized = resized.ok_or_else(Problem::not_found)?;
    let mut response = ServeFile::new(resized)
        .try_call(request)
        .await
        .map_err(|error| {
            tracing::error!(%error, "cannot read a resized image");
            Problem::new(Code::Internal)
        })?
        .map(Body::new);
    if response.status().is_success() {
        // Private: an image is only for those who can reach its library.
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=31536000, immutable"),
        );
    }
    Ok(response)
}

#[cfg(test)]
mod tests;
