//! Hiding and unhiding a track (`requirements/conventions.md` §8).

use rusqlite::params;

use super::{Effect, Error, Removed, Writer};
use crate::sync::Kind;

impl Writer<'_> {
    /// Hides `track` from the user in the library, and unlikes it: a hidden track is never liked.
    /// A track the library does not have is [`Error::NotFound`]; one hidden already stays as it
    /// was. Returns whether it was hidden now, and what the unlike took out of Liked Songs.
    pub fn hide(&mut self, track: i64) -> Result<(bool, Option<Removed>), Error> {
        let found: bool = self
            .conn
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM tracks WHERE library_id = ?1 AND id = ?2)",
            )?
            .query_row([self.library, track], |row| row.get(0))?;
        if !found {
            return Err(Error::NotFound);
        }
        // Unliked first, so a plugin applying the marks in order ends on the hide.
        let unliked = self.take_out(track)?;
        self.touch(Kind::HiddenTrack, track, 0)?;
        let hid = self
            .conn
            .prepare_cached(
                "INSERT OR IGNORE INTO hidden_tracks (user_id, library_id, track_id, hidden_at) \
                 VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![self.user, self.library, track, self.now])?
            > 0;
        if hid {
            self.report(Effect::Hidden { track, hidden: true })?;
        }
        Ok((hid, unliked))
    }

    /// Unhides `track` for the user in the library, and says whether it was hidden.
    pub fn unhide(&mut self, track: i64) -> rusqlite::Result<bool> {
        self.touch(Kind::HiddenTrack, track, 0)?;
        let deleted = self
            .conn
            .prepare_cached(
                "DELETE FROM hidden_tracks WHERE user_id = ?1 AND library_id = ?2 AND track_id = ?3",
            )?
            .execute(params![self.user, self.library, track])?
            > 0;
        if deleted {
            self.report(Effect::Hidden { track, hidden: false })?;
        }
        Ok(deleted)
    }
}
