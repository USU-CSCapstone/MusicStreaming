//! What the server makes of each row a shared write changes (`jewelcase_catalog::mutate`): the
//! account feed's row, and the mark plugins read. Call [`record`] in the transaction of the write.

use jewelcase_catalog::mutate::Effect;
use rusqlite::Connection;

use crate::db::feed::{self, Kind, Op};
use crate::db::marks::{self, Mark};

/// Records `effect` of a write by `user` in `library`.
pub fn record(tx: &Connection, user: i64, library: i64, effect: Effect) -> rusqlite::Result<()> {
    match effect {
        Effect::Playlist { id } => {
            feed::record_account(tx, user, library, Kind::Playlist, id, 0, Op::Upsert)
        }
        Effect::Item { playlist, key, kept } => {
            let op = if kept { Op::Upsert } else { Op::Delete };
            feed::record_account(tx, user, library, Kind::PlaylistItem, playlist, key, op)
        }
        Effect::Hidden { track, hidden } => {
            let (mark, op) =
                if hidden { (Mark::Hidden, Op::Upsert) } else { (Mark::Unhidden, Op::Delete) };
            marks::record(tx, user, library, [track], mark)?;
            feed::record_account(tx, user, library, Kind::HiddenTrack, track, 0, op)
        }
        Effect::Liked { track, liked } => {
            let mark = if liked { Mark::Liked } else { Mark::Unliked };
            marks::record(tx, user, library, [track], mark)
        }
        // What the `searched` hook is told is the recent searches' own (`api/recent_searches.rs`).
        Effect::Search { id, kept, .. } => {
            let op = if kept { Op::Upsert } else { Op::Delete };
            feed::record_account(tx, user, library, Kind::RecentSearch, id, 0, op)
        }
    }
}
