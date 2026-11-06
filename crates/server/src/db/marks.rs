//! The `marked` hook's outbox, `mark_events` (`0005_activity.sql`): every like, unlike, hide,
//! and unhide, written by the change itself in the same transaction, so a plugin hears of each
//! and a crash loses none (`design/hooks.md` §1.4).

use rusqlite::{Connection, Result, params};

use super::now_ms;

/// What a track now is to its user, as a mark says it: never a toggle, so hearing it twice
/// changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Liked,
    Unliked,
    Hidden,
    Unhidden,
}

impl Mark {
    /// The mark as `mark_events.kind` spells it.
    pub fn name(self) -> &'static str {
        match self {
            Mark::Liked => "liked",
            Mark::Unliked => "unliked",
            Mark::Hidden => "hidden",
            Mark::Unhidden => "unhidden",
        }
    }
}

/// Records that each of `tracks` of `library` is now `mark` to `user`. Call it in the
/// transaction that made the change, and only for tracks it changed.
pub fn record(
    tx: &Connection,
    user: i64,
    library: i64,
    tracks: impl IntoIterator<Item = i64>,
    mark: Mark,
) -> Result<()> {
    let mut insert = tx.prepare_cached(
        "INSERT INTO mark_events (user_id, library_id, track_id, kind, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    let now = now_ms();
    for track in tracks {
        insert.execute(params![user, library, track, mark.name(), now])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
