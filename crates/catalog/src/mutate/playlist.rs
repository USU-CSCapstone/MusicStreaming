//! Creating and deleting a playlist.

use jewelcase_core::playlist::Op;
use jewelcase_core::{sort_key, words};
use rusqlite::{OptionalExtension, params};

use super::edit::{plan, write};
use super::{Effect, Error, Writer};
use crate::sync::Kind;

/// A playlist that was deleted.
#[derive(Debug, PartialEq, Eq)]
pub struct Deleted {
    /// Its last version.
    pub version: i64,
    /// It was the user's Liked Songs.
    pub liked: bool,
}

impl Writer<'_> {
    /// Creates a playlist `id` for the user in the library at version 1, holding `tracks` in
    /// order, those the library still has. A blank `title` is [`Error::Invalid`], as is an `id`
    /// that is not positive, and one used already, by any user's playlist, is [`Error::IdTaken`].
    /// The playlist is reported once it is written.
    pub fn create_playlist(
        &mut self,
        id: i64,
        title: &str,
        description: Option<&str>,
        tracks: Vec<i64>,
    ) -> Result<(), Error> {
        let title = title.trim();
        if title.is_empty() {
            return Err(Error::Invalid("title must have something in it.".to_owned()));
        }
        if id <= 0 {
            return Err(Error::Invalid("id must be a positive 63-bit integer.".to_owned()));
        }
        if self.conn.prepare_cached("SELECT 1 FROM playlists WHERE id = ?1")?.exists([id])? {
            return Err(Error::IdTaken);
        }
        self.touch(Kind::Playlist, id, 0)?;
        self.conn
            .prepare_cached(
                "INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, \
             description, created_at, modified_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            )?
            .execute(params![
                id,
                self.library,
                self.user,
                title,
                words(title).join(" "),
                sort_key(title, None).as_bytes(),
                description,
                self.now
            ])?;
        // Planned as written: the playlist is new, so the work is one add.
        let add = vec![Op::Add { at: None, items: tracks }];
        let plan = plan(self.conn, self.library, id, add, self.now)?;
        write(self, id, Some(1), &plan)?;
        let (count, duration) = plan.totals.unwrap_or_default();
        self.conn
            .prepare_cached(
                "UPDATE playlists SET track_count = ?2, duration_us = ?3 WHERE id = ?1",
            )?
            .execute(params![id, count, duration])?;
        self.artwork(id)?;
        self.report(Effect::Playlist { id })?;
        Ok(())
    }

    /// Deletes the user's playlist `id` in the library and its items, and returns what it was,
    /// or `None` if it is not theirs there. What follows from deleting a Liked Songs is the
    /// caller's.
    pub fn delete_playlist(&mut self, id: i64) -> Result<Option<Deleted>, Error> {
        let (user, library) = (self.user, self.library);
        if self.is_recording() {
            let keys: Vec<i64> = self
                .conn
                .prepare_cached(
                    "SELECT i.key FROM playlists p CROSS JOIN playlist_items i \
                     ON i.playlist_id = p.id WHERE p.id = ?1 AND p.user_id = ?2 \
                     AND p.library_id = ?3",
                )?
                .query_map(params![id, user, library], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            for key in keys {
                self.touch(Kind::PlaylistItem, id, key)?;
            }
            self.touch(Kind::Playlist, id, 0)?;
        }
        let deleted = self
            .conn
            .prepare_cached(
                "DELETE FROM playlists WHERE id = ?1 AND user_id = ?2 AND library_id = ?3 \
                 RETURNING version, kind = 'liked'",
            )?
            .query_row(params![id, user, library], |row| {
                Ok(Deleted { version: row.get(0)?, liked: row.get(1)? })
            })
            .optional()?;
        if deleted.is_some() {
            self.conn
                .prepare_cached("DELETE FROM playlist_items WHERE playlist_id = ?1")?
                .execute([id])?;
        }
        Ok(deleted)
    }
}
