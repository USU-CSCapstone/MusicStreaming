//! The `artistCredit` filter: which of an artist's albums or tracks a list shows
//! (`requirements/artists.md` §2).

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;

use super::AnyId;
use super::page::Lead;

/// The spec's `ArtistCredit`: how the artist a list is filtered by is credited on its rows.
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
/// named `id`, with one `?` for the artist. Each count is an expression over `artists`, aliased
/// `ar`, giving the number of rows credited that way, counted from the credit tables or read from a
/// stored count.
pub struct Credits {
    /// Rows whose album lists the artist among its album artists.
    pub owned: &'static str,
    /// Rows a track credits the artist on.
    pub on_tracks: &'static str,
    /// The count of rows in `owned`.
    pub owned_count: &'static str,
    /// The count of rows in `on_tracks` but not `owned`.
    pub featured_count: &'static str,
}

impl ArtistCredit {
    /// The rows `credits` credits to `artist` this way, for a page to start from. Their number
    /// is the artist's count of them, 0 for an artist `library` does not hold.
    pub fn lead(
        self,
        conn: &Connection,
        library: i64,
        credits: &Credits,
        artist: AnyId,
    ) -> rusqlite::Result<Lead> {
        let Credits { owned, on_tracks, owned_count, featured_count } = credits;
        let (ids, count, times) = match self {
            ArtistCredit::Owned => (owned.to_string(), owned_count.to_string(), 1),
            ArtistCredit::Featured => {
                (format!("{on_tracks} EXCEPT {owned}"), featured_count.to_string(), 2)
            }
            ArtistCredit::Any => (
                format!("{owned} UNION {on_tracks}"),
                format!("{owned_count} + {featured_count}"),
                2,
            ),
        };
        let size = conn
            .prepare_cached(&format!(
                "SELECT {count} FROM artists ar WHERE ar.id = ?1 AND ar.library_id = ?2"
            ))?
            .query_row((artist, library), |row| row.get(0))
            .optional()?;
        let params = vec![Value::from(artist.0); times];
        Ok(Lead { ids, params, size: size.unwrap_or(0) })
    }
}
