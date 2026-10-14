//! Playback state kept across restarts (`requirements/playback.md` §6), in `player_states`.
//!
//! A report only marks its account's state as unsaved. One task saves every unsaved state
//! together every [`SAVE_EVERY`], and once more as the server stops, so the database sees one
//! write however many people are listening and however often their players report. Where an
//! account plays on through the same track, only its place changes, and that is saved only
//! every [`SAVE_PLAYING_EVERY`], sparing an SD card a write every few seconds all day.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusqlite::{Connection, params};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::{PlayerState, Players, Status};
use crate::api::Id;
use crate::db::{Database, now_ms};

/// How long a change may go unsaved: as much of their place as a crash can lose anyone.
const SAVE_EVERY: Duration = Duration::from_secs(5);

/// How long the place of an account playing on through the same track may go unsaved.
pub(super) const SAVE_PLAYING_EVERY: Duration = Duration::from_secs(60);

impl Players {
    /// Every account's state as saved, as the server starts: stopped where it was, since no
    /// device is connected yet to be playing (`requirements/realtime.md` §6). A state whose
    /// track is gone or out of its account's reach is left out (`requirements/users.md` §10).
    pub async fn load(db: &Database) -> Result<Players, crate::db::DbError> {
        let accounts = std::sync::Mutex::new(db.read(load).await?);
        Ok(Players { accounts, ..Players::default() })
    }

    /// Saves unsaved states every [`SAVE_EVERY`] until the returned task is aborted, which
    /// leaves a write already begun to finish. Call [`save`](Self::save) once more after.
    pub fn keep_saved(self: &Arc<Self>, db: Arc<Database>) -> JoinHandle<()> {
        let players = self.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(SAVE_EVERY);
            // After a stall, such as a long write by the scanner, one save covers what was missed.
            every.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                players.save_where(&db, SAVE_PLAYING_EVERY).await;
            }
        })
    }

    /// Saves every state changed since it was last saved, in one write.
    pub async fn save(&self, db: &Database) {
        self.save_where(db, Duration::ZERO).await;
    }

    /// Saves every state changed since it was last saved, in one write, except where an
    /// account plays on through the item and track it was saved at less than `playing` ago.
    pub(super) async fn save_where(&self, db: &Database, playing: Duration) {
        let now = now_ms();
        let mut rows = Vec::new();
        for (&user, state) in self.lock().iter_mut().filter(|(_, state)| state.unsaved) {
            let (Some(item), Some(library), Some(track)) =
                (state.item_id.clone(), state.library_id, state.track_id)
            else {
                continue;
            };
            let saved = state.saved.as_ref();
            if state.status == Status::Playing
                && saved.is_some_and(|s| s.0 == item && s.1 == track && s.2.elapsed() < playing)
            {
                continue;
            }
            state.saved = Some((item.clone(), track, Instant::now()));
            // Still unsaved while playing, so its place keeps being saved as it moves on.
            state.unsaved = state.status == Status::Playing;
            // Saved as far as it has played, which a restart would otherwise lose.
            rows.push((user, item, library.0, track.0, state.played_to(now)));
        }
        if rows.is_empty() {
            return;
        }
        let users: Vec<i64> = rows.iter().map(|row| row.0).collect();
        let write = db.write(move |tx| {
            let mut upsert = tx.prepare_cached(
                "INSERT OR REPLACE INTO player_states \
                     (user_id, item_id, library_id, track_id, position_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (user, item, library, track, position_ms) in rows {
                // One account's failing, as when its library was deleted under it, must not keep
                // everyone else's from being saved. SQLite undoes only the statement that failed.
                if let Err(error) = upsert.execute(params![user, item, library, track, position_ms])
                {
                    tracing::warn!(%error, user, "cannot save an account's playback state");
                }
            }
            Ok(())
        });
        if let Err(error) = write.await {
            tracing::warn!(%error, "cannot save playback state");
            // Tried again at the next save, not left until each account's next change.
            let mut accounts = self.lock();
            for user in users {
                if let Some(state) = accounts.get_mut(&user) {
                    state.unsaved = true;
                    state.saved = None;
                }
            }
        }
    }
}

fn load(conn: &Connection) -> rusqlite::Result<HashMap<i64, PlayerState>> {
    let mut select = conn.prepare(
        "SELECT s.user_id, s.item_id, s.library_id, s.track_id, s.position_ms \
         FROM player_states s JOIN users u ON u.id = s.user_id \
         WHERE u.status = 'active' \
           AND EXISTS (SELECT 1 FROM tracks WHERE id = s.track_id AND library_id = s.library_id) \
           AND (u.role <> 'user' OR EXISTS \
                (SELECT 1 FROM library_access WHERE user_id = u.id AND library_id = s.library_id))",
    )?;
    let rows = select.query_map([], |row| {
        let state = PlayerState {
            item_id: Some(row.get(1)?),
            library_id: Some(Id(row.get(2)?)),
            track_id: Some(Id(row.get(3)?)),
            position_ms: row.get(4)?,
            ..PlayerState::stopped()
        };
        Ok((row.get(0)?, state))
    })?;
    rows.collect()
}
