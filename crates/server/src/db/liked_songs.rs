//! Each user's Liked Songs: the playlist in each library whose items are their likes
//! (`requirements/playlists.md` §11, `design/database.md` §6).

use rusqlite::{Connection, Result};

use super::feed::{self, Kind, Op};
use super::reach::reaches;
use super::{new_id, now_ms};

#[cfg(test)]
use jewelcase_catalog::liked::TITLE;

/// Makes a Liked Songs, empty and pinned, for every user in every library they reach that has
/// none and that they did not delete theirs in, so every user has one from the start. Called
/// wherever a user may come to reach a library: as one is created, and as the owner is.
pub fn ensure(conn: &Connection) -> Result<()> {
    let missing: Vec<(i64, i64)> = conn
        .prepare_cached(concat!(
            "SELECT u.id, l.id FROM users u CROSS JOIN libraries l WHERE ",
            reaches!(admin: "u.role <> 'user'", user: "u.id", library: "l.id"),
            " AND NOT EXISTS (SELECT 1 FROM playlists p \
               WHERE p.user_id = u.id AND p.library_id = l.id AND p.kind = 'liked') \
             AND NOT EXISTS (SELECT 1 FROM liked_songs_deleted d \
               WHERE d.user_id = u.id AND d.library_id = l.id)"
        ))?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_>>()?;
    let now = now_ms();
    for (user, library) in missing {
        create(conn, user, library, now)?;
    }
    Ok(())
}

pub use jewelcase_catalog::liked::find;

/// Makes `user`'s Liked Songs in `library`, empty and pinned, created at `now`, and returns its
/// ID. They have none there.
pub fn create(conn: &Connection, user: i64, library: i64, now: i64) -> Result<i64> {
    let id = new_id()?;
    jewelcase_catalog::liked::create(conn, user, library, id, now)?;
    // Their devices keep it, as any playlist of theirs.
    feed::record_account(conn, user, library, Kind::Playlist, id, 0, Op::Upsert)?;
    Ok(id)
}

#[cfg(test)]
mod tests;
