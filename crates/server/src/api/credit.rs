//! The `artistCredit` filter: which of an artist's albums or tracks a list shows
//! (`requirements/artists.md` §2).

use rusqlite::types::Value;
use serde::Deserialize;

use super::Id;

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

impl ArtistCredit {
    /// A query for the IDs credited to `artist` this way, and its parameters. `owned` and
    /// `on_tracks` each select the IDs one way, with a `?` for the artist. A malformed ID
    /// matches nothing, like an unknown one: IDs are never negative.
    pub fn query(self, owned: &str, on_tracks: &str, artist: &str) -> (String, Vec<Value>) {
        let artist = Value::Integer(Id::canonical(artist).map_or(-1, |Id(id)| id));
        let (sql, times) = match self {
            ArtistCredit::Owned => (owned.to_owned(), 1),
            ArtistCredit::Featured => (format!("{on_tracks} EXCEPT {owned}"), 2),
            ArtistCredit::Any => (format!("{owned} UNION {on_tracks}"), 2),
        };
        (sql, vec![artist; times])
    }
}
