//! Narrowing a search to one album, artist, playlist or tag (`within`, `requirements/search.md`
//! §5): the rows of each kind it holds, as the server and a device read them alike.

use std::collections::HashSet;

use jewelcase_core::Kind;
use rusqlite::{Connection, OptionalExtension, params_from_iter};

use crate::credit::ArtistCredit;
use crate::search::{self, Hid};
use crate::tag_filter::TagFilter;
use crate::{AnyId, Invalid, albums, tracks};

/// What a search is narrowed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Within {
    Album(i64),
    Artist(i64),
    Playlist(i64),
    Tag(i64),
}

/// What each kind of row a container holds, in `Kind::ALL` order.
pub type Held = [HashSet<i64>; Kind::ALL.len()];

impl Within {
    /// The container `text` names, as `album:ID`, `artist:ID`, `playlist:ID` or `tag:ID`, or
    /// `None` for an ID that is no ID, which names nothing there is. Any other shape is
    /// [`Invalid`].
    pub fn parse(text: &str) -> Result<Option<Within>, Invalid> {
        let invalid =
            || Invalid("within must be album:, artist:, playlist: or tag: and an ID".to_owned());
        let (kind, id) = text.split_once(':').ok_or_else(invalid)?;
        let id = AnyId::from(id).0;
        let within = match kind {
            "album" => Within::Album,
            "artist" => Within::Artist,
            "playlist" => Within::Playlist,
            "tag" => Within::Tag,
            _ => return Err(invalid()),
        };
        Ok(id.map(within))
    }

    /// The kind of result the container is, which the caller may have hidden whole.
    pub fn kind(self) -> Kind {
        match self {
            Within::Album(_) => Kind::Album,
            Within::Artist(_) => Kind::Artist,
            Within::Playlist(_) => Kind::Playlist,
            Within::Tag(_) => Kind::Tag,
        }
    }

    /// Its ID.
    pub fn id(self) -> i64 {
        match self {
            Within::Album(id) | Within::Artist(id) | Within::Playlist(id) | Within::Tag(id) => id,
        }
    }
}

/// What `within` holds for `user`'s search, whose hidden rows `hid` says, or `None` if it is out
/// of their reach, as one that is not there is: not in the library, not their own playlist, or an
/// album, artist or tag every track of which they hid (`requirements/users.md` §10).
pub fn reached(
    conn: &Connection,
    user: i64,
    library: i64,
    within: Within,
    hid: &Hid,
) -> rusqlite::Result<Option<Held>> {
    if hid.contains(within.kind(), within.id()) {
        return Ok(None);
    }
    read(conn, user, library, within)
}

/// The rows of each kind, in `Kind::ALL` order, that `within` holds in `library` as `user` reaches
/// it, or `None` if it is not there: an album, artist or tag the library lacks, or a playlist
/// that is not the user's own in it. An album holds its tracks; an artist the tracks and albums
/// credited to them, owned or featured; a playlist its tracks; and a tag the tracks, albums and
/// artists that carry it, as a filter by it reads them. A container holds nothing of any other
/// kind, so a search within it finds none.
pub fn read(
    conn: &Connection,
    user: i64,
    library: i64,
    within: Within,
) -> rusqlite::Result<Option<Held>> {
    let exists = |table: &str| -> rusqlite::Result<bool> {
        let sql =
            format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE library_id = ?1 AND id = ?2)");
        conn.prepare_cached(&sql)?.query_row([library, within.id()], |row| row.get(0))
    };
    let ids = |sql: &str, params: &[rusqlite::types::Value]| -> rusqlite::Result<HashSet<i64>> {
        conn.prepare_cached(sql)?.query_map(params_from_iter(params), |row| row.get(0))?.collect()
    };
    let mut sets: Held = Default::default();
    match within {
        Within::Album(album) => {
            if !exists("albums")? {
                return Ok(None);
            }
            let sql = "SELECT id FROM tracks WHERE library_id = ? AND album_id = ?";
            sets[Kind::Track as usize] = ids(sql, &[library.into(), album.into()])?;
        }
        Within::Artist(artist) => {
            if !exists("artists")? {
                return Ok(None);
            }
            let artist = AnyId(Some(artist));
            for (kind, credits) in
                [(Kind::Track, &tracks::CREDITS), (Kind::Album, &albums::CREDITS)]
            {
                let lead = ArtistCredit::Any.lead(conn, library, credits, artist)?;
                sets[kind as usize] = ids(&format!("SELECT id FROM ({})", lead.ids), &lead.params)?;
            }
        }
        Within::Playlist(playlist) => {
            let owned: Option<i64> = conn
                .prepare_cached(
                    "SELECT id FROM playlists WHERE id = ?1 AND library_id = ?2 AND user_id = ?3",
                )?
                .query_row([playlist, library, user], |row| row.get(0))
                .optional()?;
            if owned.is_none() {
                return Ok(None);
            }
            let sql = "SELECT track_id FROM playlist_items WHERE playlist_id = ?";
            sets[Kind::Track as usize] = ids(sql, &[playlist.into()])?;
        }
        Within::Tag(tag) => {
            if !exists("tags")? {
                return Ok(None);
            }
            let filter = TagFilter { any: vec![AnyId(Some(tag))], ..TagFilter::default() };
            let kinds = [Kind::Track, Kind::Album, Kind::Artist];
            sets = search::tagged(conn, library, &filter, &kinds)?;
        }
    }
    Ok(Some(sets))
}
