//! `libraryChanged`: tells every connected account that reaches a library that its change feed
//! moved, so a device pulls what it missed (`requirements/realtime.md` §4).
//!
//! The database says when the feed has moved, after the write that moved it has committed. A
//! library is hinted at most once each [`WINDOW`]; a change inside one is hinted when it ends,
//! carrying the latest position, so the last change of a burst is never left unhinted.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until};
use tracing::warn;

use super::{Hub, ServerMessage};
use crate::api::Id;
use crate::db::{Database, reaches};

/// The shortest time between two hints for one library.
pub(super) const WINDOW: Duration = Duration::from_secs(1);

/// Spawns the task that sends `libraryChanged` for every change committed from now on. The hints
/// stop when the returned task is aborted or the database is dropped.
pub fn start(db: Arc<Database>, hub: Arc<Hub>) -> JoinHandle<()> {
    // Before the task is polled, so a change committed meanwhile is hinted.
    let mut feed = db.follow_feed();
    let mut seen = *feed.borrow_and_update();
    tokio::spawn(async move {
        let mut throttle = Throttle::default();
        loop {
            let deadline = throttle.next_deadline();
            let due = async {
                match deadline {
                    Some(at) => sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            let hints = tokio::select! {
                moved = feed.changed() => {
                    if moved.is_err() {
                        return;
                    }
                    let now = Instant::now();
                    match moved_since(&db, seen).await {
                        Ok(moved) => {
                            seen = moved.iter().map(|&(_, seq)| seq).fold(seen, i64::max);
                            moved.into_iter().filter_map(|(library, seq)| {
                                throttle.moved(now, library, seq).map(|seq| (library, seq))
                            }).collect()
                        }
                        Err(error) => {
                            warn!(%error, "cannot read the change feed");
                            Vec::new()
                        }
                    }
                }
                () = due => throttle.due(Instant::now()),
            };
            for (library, seq) in hints {
                send(&db, &hub, library, seq).await;
            }
        }
    })
}

/// Each library with changes after `seen`, and its latest position.
async fn moved_since(db: &Database, seen: i64) -> Result<Vec<(i64, i64)>, crate::db::DbError> {
    db.read(move |conn| {
        conn.prepare_cached(
            "SELECT library_id, max(seq) FROM library_changes WHERE seq > ?1 GROUP BY library_id",
        )?
        .query_map([seen], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
    })
    .await
}

/// Sends `libraryChanged` to each connection of each active account that reaches `library`.
async fn send(db: &Database, hub: &Hub, library: i64, seq: i64) {
    let users = db
        .read(move |conn| {
            conn.prepare_cached(&format!(
                "SELECT u.id FROM users AS u WHERE u.status = 'active' AND {}",
                reaches!(admin: "u.role <> 'user'", user: "u.id", library: "?1")
            ))?
            .query_map([library], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await;
    match users {
        Ok(users) => {
            let message = ServerMessage::LibraryChanged { library_id: Id(library), seq };
            for user in users {
                hub.broadcast(user, &message);
            }
        }
        Err(error) => warn!(%error, "cannot find who reaches a library to hint"),
    }
}

/// When each library was last hinted, and the latest position it has moved to since.
#[derive(Default)]
pub(super) struct Throttle(HashMap<i64, Window>);

struct Window {
    /// When the library was hinted last.
    opened: Instant,
    /// The latest position of changes made since, not yet hinted.
    pending: Option<i64>,
}

impl Throttle {
    /// Notes that `library` moved to `seq` at `now`. Returns the position to hint at once if the
    /// library's window has ended, and otherwise holds it for [`due`](Self::due) and returns `None`.
    pub(super) fn moved(&mut self, now: Instant, library: i64, seq: i64) -> Option<i64> {
        match self.0.get_mut(&library) {
            Some(window) if now < window.opened + WINDOW => {
                window.pending = window.pending.max(Some(seq));
                None
            }
            _ => {
                self.0.insert(library, Window { opened: now, pending: None });
                Some(seq)
            }
        }
    }

    /// The libraries to hint at `now`, with their latest positions: those held whose window has
    /// ended.
    pub(super) fn due(&mut self, now: Instant) -> Vec<(i64, i64)> {
        let mut due = Vec::new();
        for (&library, window) in &mut self.0 {
            if now >= window.opened + WINDOW
                && let Some(seq) = window.pending.take()
            {
                window.opened = now;
                due.push((library, seq));
            }
        }
        due
    }

    /// When the next held position can be hinted, if any are held.
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        let held = self.0.values().filter(|window| window.pending.is_some());
        held.map(|window| window.opened + WINDOW).min()
    }
}
