//! Liking and unliking a track: a like puts a track at the top of the user's Liked Songs, made
//! again if they deleted it, and an unlike takes it out (`requirements/playlists.md` §11).
//! Neither is checked against a version: each says whether a track is liked, not where it sits,
//! so it applies whatever else changed the playlist. Nor does either read the playlist: each
//! finds the track's rows through `playlist_items_by_track`, writes only those, and moves the
//! totals by them, so the writer is held as briefly in a Liked Songs of thousands as in an empty
//! one. Liking a hidden track unhides it, and hiding one unlikes it, so the two never contradict
//! each other (`requirements/conventions.md` §8).

use jewelcase_core::playlist::Op;
use rusqlite::{Connection, OptionalExtension, params};

use super::edit::{plan, write};
use super::{Effect, Error, KEY_GAP, Writer};
use crate::liked;
use crate::sync::Kind;

/// What a like did.
#[derive(Debug, PartialEq, Eq)]
pub struct Liked {
    /// The track was hidden, and is not now.
    pub unhidden: bool,
    /// The Liked Songs the track was added to; none when it was in it already.
    pub added_to: Option<i64>,
}

/// What an unlike took out of a Liked Songs.
#[derive(Debug, PartialEq, Eq)]
pub struct Removed {
    pub playlist: i64,
    /// Where the track was, in the playlist's order, once for each time it was there.
    pub positions: Vec<usize>,
}

/// Whether playlist `id` of `library` holds `track`, found through `playlist_items_by_track`.
pub(super) fn holds(
    conn: &Connection,
    library: i64,
    id: i64,
    track: i64,
) -> rusqlite::Result<bool> {
    conn.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM playlist_items INDEXED BY playlist_items_by_track \
         WHERE library_id = ?1 AND track_id = ?2 AND playlist_id = ?3)",
    )?
    .query_row(params![library, track, id], |row| row.get(0))
}

/// The duration of `track`, or `None` if `library` has no such track.
fn duration(conn: &Connection, library: i64, track: i64) -> rusqlite::Result<Option<i64>> {
    conn.prepare_cached("SELECT duration_us FROM tracks WHERE library_id = ?1 AND id = ?2")?
        .query_row([library, track], |row| row.get(0))
        .optional()
}

impl Writer<'_> {
    /// Puts `track` at the top of the user's Liked Songs, unless it is in it already, making the
    /// playlist, pinned, under `fresh_id` if they have none, and unhides it. A track the library
    /// does not have is [`Error::NotFound`]. With no Liked Songs and no `fresh_id` it is
    /// [`Error::Invalid`].
    pub fn like(&mut self, track: i64, fresh_id: Option<i64>) -> Result<Liked, Error> {
        let duration = duration(self.conn, self.library, track)?.ok_or(Error::NotFound)?;
        let unhidden = self.unhide(track)?;
        let (id, version) = match liked::find(self.conn, self.user, self.library)? {
            Some(found) => found,
            None => {
                let id = fresh_id.ok_or_else(|| {
                    Error::Invalid("A like needs an ID for the Liked Songs it makes.".to_owned())
                })?;
                self.touch(Kind::Playlist, id, 0)?;
                liked::create(self.conn, self.user, self.library, id, self.now)?;
                // Their devices keep it, as any playlist of theirs.
                self.report(Effect::Playlist { id })?;
                (id, 1)
            }
        };
        if holds(self.conn, self.library, id, track)? {
            return Ok(Liked { unhidden, added_to: None });
        }
        self.put_first((id, version), track)?;
        self.changed(id, (1, duration))?;
        self.report(Effect::Liked { track, liked: true })?;
        Ok(Liked { unhidden, added_to: Some(id) })
    }

    /// Takes `track` out of the user's Liked Songs: nothing when it is not in it, or they have
    /// none. Returns what it took out.
    pub fn take_out(&mut self, track: i64) -> Result<Option<Removed>, Error> {
        let Some((id, _)) = liked::find(self.conn, self.user, self.library)? else {
            return Ok(None);
        };
        // Each probe finds the track through `playlist_items_by_track`, then counts the items
        // before it in key order.
        let mut found: Vec<(i64, i64)> = self
            .conn
            .prepare_cached(
                "SELECT i.key, (SELECT count(*) FROM playlist_items j \
                 WHERE j.playlist_id = i.playlist_id AND j.key < i.key) \
                 FROM playlist_items i INDEXED BY playlist_items_by_track \
                 WHERE i.library_id = ?1 AND i.track_id = ?2 AND i.playlist_id = ?3",
            )?
            .query_map(params![self.library, track, id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        if found.is_empty() {
            return Ok(None);
        }
        found.sort_unstable();
        for &(key, _) in &found {
            self.touch(Kind::PlaylistItem, id, key)?;
            self.conn
                .prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1 AND key = ?2")?
                .execute(params![id, key])?;
            self.report(Effect::Item { playlist: id, key, kept: false })?;
        }
        // Retention keeps a track any playlist holds, so it is still there to subtract.
        let duration = duration(self.conn, self.library, track)?.unwrap_or(0);
        let count = i64::try_from(found.len()).unwrap_or(i64::MAX);
        self.changed(id, (-count, -count.saturating_mul(duration)))?;
        self.report(Effect::Liked { track, liked: false })?;
        let positions = found
            .into_iter()
            .map(|(_, position)| usize::try_from(position).unwrap_or_default())
            .collect();
        Ok(Some(Removed { playlist: id, positions }))
    }

    /// Puts `track` at the top of the user's Liked Songs `id`, at `version`, under a key
    /// [`KEY_GAP`] before the first: one row written, and none read but the first. Only when no
    /// key is left before the first is the playlist renumbered, as an edit would.
    fn put_first(&mut self, (id, version): (i64, i64), track: i64) -> Result<(), Error> {
        let first: Option<i64> = self
            .conn
            .prepare_cached("SELECT min(key) FROM playlist_items WHERE playlist_id = ?1")?
            .query_row([id], |row| row.get(0))?;
        let Some(key) = first.map_or(Some(KEY_GAP), |first| first.checked_sub(KEY_GAP)) else {
            let add = Op::Add { at: Some(0), items: vec![track] };
            let plan = plan(self.conn, self.library, id, vec![add], self.now)?;
            return write(self, id, Some(version), &plan);
        };
        self.touch(Kind::PlaylistItem, id, key)?;
        self.conn
            .prepare_cached(
                "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![id, key, self.library, track, self.now])?;
        self.report(Effect::Item { playlist: id, key, kept: true })?;
        Ok(())
    }

    /// Moves the user's Liked Songs `id` on a version once its items changed, adding
    /// `(count, duration)` to its totals.
    fn changed(&mut self, id: i64, (count, duration): (i64, i64)) -> rusqlite::Result<()> {
        self.touch(Kind::Playlist, id, 0)?;
        self.conn
            .prepare_cached(
                "UPDATE playlists SET track_count = max(track_count + ?2, 0), \
                 duration_us = max(duration_us + ?3, 0), version = version + 1, modified_at = ?4 \
                 WHERE id = ?1",
            )?
            .execute(params![id, count, duration, self.now])?;
        self.report(Effect::Playlist { id })
    }
}
