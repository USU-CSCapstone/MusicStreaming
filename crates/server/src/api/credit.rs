//! The `artistCredit` filter: which of an artist's albums or tracks a list shows
//! (`requirements/artists.md` §2).

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;

use super::AnyId;
use super::page::Lead;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtistCredit {
    /// Owned or featured.
    #[default]
    Any,
    /// Credited as an album artist.
    Owned,
    /// Credited on a track, but not as its album's artist.
    Featured,
}

/// How a list's rows are credited to an artist. Each query selects the rows' IDs in a column
/// named `id`, with one `?` for the artist. Each count is an expression over `artists` that the
/// scanner keeps equal to the number of rows credited that way.
pub struct Credits {
    /// Rows whose album lists the artist among its album artists.
    pub owned: &'static str,
    /// Rows a track credits the artist on.
    pub on_tracks: &'static str,
    pub owned_count: &'static str,
    pub featured_count: &'static str,
    pub any_count: &'static str,
}

impl ArtistCredit {
    /// The rows `credits` credits to `artist` this way, for a page to start from. Their number
    /// is the artist's stored count, 0 for an artist `library` does not hold.
    pub fn lead(
        self,
        conn: &Connection,
        library: i64,
        credits: &Credits,
        artist: AnyId,
    ) -> rusqlite::Result<Lead> {
        let Credits { owned, on_tracks, .. } = credits;
        let (ids, count, times) = match self {
            ArtistCredit::Owned => (owned.to_string(), credits.owned_count, 1),
            ArtistCredit::Featured => {
                (format!("{on_tracks} EXCEPT {owned}"), credits.featured_count, 2)
            }
            ArtistCredit::Any => (format!("{owned} UNION {on_tracks}"), credits.any_count, 2),
        };
        let size = conn
            .prepare_cached(&format!(
                "SELECT {count} FROM artists WHERE id = ?1 AND library_id = ?2"
            ))?
            .query_row((artist, library), |row| row.get(0))
            .optional()?;
        let params = vec![Value::from(artist.0); times];
        Ok(Lead { ids, params, size: size.unwrap_or(0) })
    }
}
