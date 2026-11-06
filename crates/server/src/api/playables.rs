//! Playables: the things a request names to be played or added as a run of tracks, and the
//! tracks each stands for (`api/openapi.yaml`, `Playable`).
//!
//! Expansion is scoped to the caller: a track, album or artist must be in the request's library,
//! and a playlist must be the caller's own in it. Anything else is as absent as an ID that never
//! was (`requirements/users.md` §10), answered `422` here because the request, not its path,
//! names it.
//!
//! An album, artist or playlist stands for the tracks its page shows, so without those the
//! caller hid (`requirements/conventions.md` §8). A track named on its own is itself, hidden or
//! not: the request asked for it.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;

use super::tracks::{CREDITS, TRACK_NUMBER};
use super::{AnyId, Problem, hidden};

/// The spec's `Playable` as a request sends it. Only the kinds of library content are
/// understood; the check is made when it is expanded.
#[derive(Debug, Clone, Deserialize)]
pub struct Playable {
    #[serde(rename = "type")]
    kind: String,
    id: Option<AnyId>,
}

/// An album's tracks in the order the album lists them: by disc, then track number, with
/// unnumbered tracks last, the order of `tracks_by_album_order`.
fn album_tracks() -> String {
    format!(
        "SELECT t.id FROM tracks t WHERE t.library_id = ?1 AND t.album_id = ?2 \
         ORDER BY t.disc_number, {TRACK_NUMBER}, t.id"
    )
}

/// A playlist's tracks in its order, each as often as it is there, missing ones included.
const PLAYLIST_TRACKS: &str =
    "SELECT track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY key";

/// The tracks `playables` stand for, in request order, each kind in its own order and a track
/// as often as it is named, without those `user` hid in an album, artist or playlist. A track,
/// album or artist is looked up in `library`, a playlist among `user`'s own in it. Fails with `422` for a playable that is not there, and for the kinds not
/// supported yet.
pub fn expand(
    conn: &Connection,
    user: i64,
    library: i64,
    playables: &[Playable],
) -> Result<Vec<i64>, Problem> {
    let mut tracks = Vec::new();
    let collections = playables.iter().any(|playable| playable.kind != "track");
    let hid = if collections { hidden::ids(conn, user, library)? } else { HashSet::new() };
    for playable in playables {
        let id = playable.id.and_then(|id| id.0);
        match playable.kind.as_str() {
            "track" => {
                require(exists(conn, "tracks", library, id)?, "track")?;
                tracks.extend(id);
            }
            "album" => {
                require(exists(conn, "albums", library, id)?, "album")?;
                collect(conn, &album_tracks(), params![library, id], &hid, &mut tracks)?;
            }
            "artist" => {
                require(exists(conn, "artists", library, id)?, "artist")?;
                collect(conn, &artist_tracks(), params![library, id], &hid, &mut tracks)?;
            }
            "playlist" => {
                let own = conn
                    .prepare_cached(
                        "SELECT 1 FROM playlists WHERE id = ?1 AND user_id = ?2 AND library_id = ?3",
                    )?
                    .query_row(params![id, user, library], |_| Ok(()))
                    .optional()?
                    .is_some();
                require(own, "playlist")?;
                collect(conn, PLAYLIST_TRACKS, params![id], &hid, &mut tracks)?;
            }
            "external" => {
                return Err(Problem::invalid("External tracks are not supported yet."));
            }
            _ => {
                return Err(Problem::invalid("type must be track, album, artist, or playlist."));
            }
        }
    }
    Ok(tracks)
}

/// An artist's tracks, owned or featured, in album order as the web client adds an artist
/// (`listTracks` with `sort=album`): by the album's sort key, then the album, disc and number.
fn artist_tracks() -> String {
    format!(
        "SELECT t.id FROM tracks t WHERE t.library_id = ?1 \
         AND t.id IN ({} UNION {}) \
         ORDER BY t.album_sort_key, t.album_id, t.disc_number, {TRACK_NUMBER}, t.id",
        CREDITS.owned.replace('?', "?2"),
        CREDITS.on_tracks.replace('?', "?2"),
    )
}

/// Whether `table` has row `id` in `library`.
fn exists(conn: &Connection, table: &str, library: i64, id: Option<i64>) -> rusqlite::Result<bool> {
    let sql = format!("SELECT 1 FROM {table} WHERE id = ?1 AND library_id = ?2");
    Ok(conn.prepare_cached(&sql)?.query_row(params![id, library], |_| Ok(())).optional()?.is_some())
}

/// `422` naming `kind` unless `found`.
fn require(found: bool, kind: &str) -> Result<(), Problem> {
    if found { Ok(()) } else { Err(Problem::invalid(format!("This library has no such {kind}."))) }
}

/// Appends the first column of every row `sql` returns to `tracks`, but those in `hid`.
fn collect(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
    hid: &HashSet<i64>,
    tracks: &mut Vec<i64>,
) -> rusqlite::Result<()> {
    for track in conn.prepare_cached(sql)?.query_map(params, |row| row.get::<_, i64>(0))? {
        let track = track?;
        if !hid.contains(&track) {
            tracks.push(track);
        }
    }
    Ok(())
}
