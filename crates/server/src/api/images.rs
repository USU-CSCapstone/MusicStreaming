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
use super::{Code, Id, Problem};
use crate::db::{Database, libraries};

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

/// The spec's `getImage`: the image resized to the first of [`SIZES`] at or above `size`
/// ([`DEFAULT_SIZE`] if not given), or to the first that holds the whole image if that is
/// smaller. An image under a root no longer in its library, or whose
/// file is missing, is `404`.
pub async fn get(
    State(db): State<Arc<Database>>,
    State(images): State<Arc<Images>>,
    Scope { library, .. }: Scope,
    Path((_, Id(image))): Path<(Id, Id)>,
    Query(query): Query<ImageQuery>,
    request: Request,
) -> Result<Response, Problem> {
    let size = match query.size.unwrap_or(DEFAULT_SIZE) {
        requested @ 16..=4096 => SIZES.into_iter().find(|&size| size >= requested),
        _ => None,
    }
    .ok_or_else(|| Problem::invalid("size must be from 16 to 4096"))?;
    let (hash, longest, source) = db
        .try_read(move |conn| -> Result<_, Problem> {
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
            let file =
                libraries::file(conn, library, root, &path)?.ok_or_else(Problem::not_found)?;
            Ok((hash, longest, file))
        })
        .await?;
    let size =
        SIZES.into_iter().find(|&whole| whole >= longest).map_or(size, |whole| size.min(whole));

    let resized = images.resized(library, &hash, size, source).await.map_err(|error| {
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
