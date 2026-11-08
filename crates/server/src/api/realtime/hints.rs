//! `libraryChanged` and `accountChanged`: tell every connected account that reaches a library
//! that its change feed moved, and an account that its own feed moved, so a device pulls what it
//! missed (`requirements/realtime.md` §4).
//!
//! The database says when a feed has moved, after the write that moved it has committed. A
//! library, and an account, is hinted at most once each [`WINDOW`]; a change inside one is hinted
//! when it ends, carrying the latest position, so the last change of a burst is never left
//! unhinted. One [`Throttle`] per kind serves both, an account's ID being its key.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until};
use tracing::warn;

use super::{Hub, ServerMessage};
use crate::api::Id;
use crate::db::{Database, reaches};

/// The shortest time between two hints for one library or account.
pub(super) const WINDOW: Duration = Duration::from_secs(1);

/// Spawns the task that sends `libraryChanged` and `accountChanged` for every change committed
/// from now on. The hints stop when the returned task is aborted or the database is dropped.
pub fn start(db: Arc<Database>, hub: Arc<Hub>) -> JoinHandle<()> {
    // Before the task is polled, so a change committed meanwhile is hinted.
    let mut library_feed = db.follow_feed();
    let mut library_seen = *library_feed.borrow_and_update();
    let mut account_feed = db.follow_account_feed();
    let mut account_seen = *account_feed.borrow_and_update();
    tokio::spawn(async move {
        let (mut libraries, mut accounts) = (Throttle::default(), Throttle::default());
        loop {
            let deadline =
                [libraries.next_deadline(), accounts.next_deadline()].into_iter().flatten().min();
            let due = async {
                match deadline {
                    Some(at) => sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            let (library_hints, account_hints) = tokio::select! {
                moved = library_feed.changed() => {
                    if moved.is_err() {
                        return;
                    }
                    let now = Instant::now();
                    let moved = moved_since(&db, LIBRARIES, library_seen).await;
                    library_seen = advance(library_seen, &moved);
                    (hinted(&mut libraries, now, moved), Vec::new())
                }
                moved = account_feed.changed() => {
                    if moved.is_err() {
                        return;
                    }
                    let now = Instant::now();
                    let moved = moved_since(&db, ACCOUNTS, account_seen).await;
                    account_seen = advance(account_seen, &moved);
                    (Vec::new(), hinted(&mut accounts, now, moved))
                }
                () = due => {
                    let now = Instant::now();
                    (libraries.due(now), accounts.due(now))
                }
            };
            for (library, seq) in library_hints {
                send(&db, &hub, library, seq).await;
            }
            for (user, seq) in account_hints {
                hub.broadcast(user, &ServerMessage::AccountChanged { seq });
            }
        }
    })
}

/// The query for each library with changes after `?1`, and its latest position.
const LIBRARIES: &str =
    "SELECT library_id, max(seq) FROM library_changes WHERE seq > ?1 GROUP BY library_id";

/// The query for each account with changes after `?1`, and its latest position.
const ACCOUNTS: &str =
    "SELECT user_id, max(seq) FROM account_changes WHERE seq > ?1 GROUP BY user_id";

/// Each library or account `sql` finds with changes after `seen`, and its latest position. An
/// error is logged and reads as no change.
async fn moved_since(db: &Database, sql: &'static str, seen: i64) -> Vec<(i64, i64)> {
    let moved = db
        .read(move |conn| {
            conn.prepare_cached(sql)?
                .query_map([seen], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .await;
    moved.unwrap_or_else(|error| {
        warn!(%error, "cannot read a change feed");
        Vec::new()
    })
}

/// The latest position `seen` moved to, given what [`moved_since`] found.
fn advance(seen: i64, moved: &[(i64, i64)]) -> i64 {
    moved.iter().map(|&(_, seq)| seq).fold(seen, i64::max)
}

/// What `throttle` lets through of `moved` at `now`: the positions to hint at once.
fn hinted(throttle: &mut Throttle, now: Instant, moved: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    moved
        .into_iter()
        .filter_map(|(key, seq)| throttle.moved(now, key, seq).map(|seq| (key, seq)))
        .collect()
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

/// When each library (or account) was last hinted, and the latest position it has moved to since.
#[derive(Default)]
pub(super) struct Throttle(HashMap<i64, Window>);

/// One library's, or account's, place in a [`Throttle`].
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
