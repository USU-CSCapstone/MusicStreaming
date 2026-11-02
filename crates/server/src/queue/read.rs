//! Reading an account's lists and pages of rows for the API (`design/queue.md` §7, §8, §9).

use std::collections::HashMap;
use std::sync::Arc;

use jewelcase_core::Repeat;
use rusqlite::{Connection, MAIN_DB, OptionalExtension, params};

use super::store::{Held, Queues};
use crate::api::Uuid;
use crate::db::{Database, DbError};

/// Which part of the queue a page of rows is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Section {
    /// The Manual Queue followed by the rows still to play.
    UpNext,
    /// The plays already made, newest first.
    History,
}

/// A row as taken under the lock, before its track is read: everything but the track.
pub struct Slot {
    /// The row's item, `<key>.<position>`.
    pub item: String,
    /// The library of the row's list.
    pub library_id: String,
    /// The play's number, for a History row.
    pub play: Option<u64>,
    /// The row's list by hash and the position in it, or `None` where the source has no list.
    list: Option<(String, u64)>,
}

/// A page of rows as the server's memory describes it, with the lists it reads from held.
pub struct Page {
    pub queue_id: Uuid,
    pub version: u32,
    pub current: Option<Slot>,
    pub total: u64,
    pub manual: u64,
    pub shuffled: bool,
    pub repeat: Repeat,
    pub rows: Vec<Slot>,
    /// The page's lists that are held in memory, so reading them needs no lock.
    held: HashMap<String, Arc<[u64]>>,
}

impl Queues {
    /// The `limit` rows of `section` from `offset` of `user`'s active queue, and its current
    /// row, as memory has them; `None` if they have no queue. Takes only positions under the
    /// lock: [`Queues::tracks`] reads the tracks after it is released.
    pub fn page(&self, user: i64, section: Section, offset: u64, limit: u64) -> Option<Page> {
        let accounts = self.lock();
        let account = accounts.get(&user)?;
        let state = &account.state;
        let slot = |row: jewelcase_core::Row<'_>, play: Option<u64>| {
            let source = state.sources.iter().find(|source| source.key == row.source)?;
            let list = source.hash.clone().map(|hash| (hash, row.position));
            Some(Slot {
                item: row.item.to_string(),
                library_id: source.library_id.clone(),
                play,
                list,
            })
        };
        let (total, manual) = match section {
            Section::UpNext => (state.up_next_len(), state.manual.len()),
            Section::History => (state.history_len(), 0),
        };
        let end = total.min(offset.saturating_add(limit));
        let rows: Vec<Slot> = (offset..end)
            .filter_map(|index| match section {
                Section::UpNext => slot(state.up_next_row(index)?, None),
                Section::History => {
                    let played = state.history_row(index)?;
                    slot(played.row, Some(played.play))
                }
            })
            .collect();
        let current = state.current_row().and_then(|row| slot(row, None));
        let held = rows
            .iter()
            .chain(&current)
            .filter_map(|slot| slot.list.as_ref())
            .filter_map(|(hash, _)| Some((hash.clone(), account.lists.get(hash)?.tracks.clone())))
            .collect();
        Some(Page {
            queue_id: account.queue_id.clone(),
            version: account.version,
            current,
            total,
            manual,
            shuffled: state.shuffle.is_some(),
            repeat: state.repeat,
            rows,
            held,
        })
    }

    /// The track of the page's `current` row, if it has one, and then of each of its rows, or
    /// `None` where the row's list can't be read. Reads from the held lists, else 8 bytes of the
    /// stored list by incremental BLOB I/O, so no list is read whole. Takes no lock.
    pub async fn tracks(
        db: &Database,
        user: i64,
        page: &Page,
    ) -> Result<Vec<Option<u64>>, DbError> {
        let slots = page.current.iter().chain(&page.rows);
        let mut found: Vec<Option<u64>> = Vec::new();
        let mut wanted: Vec<Option<(String, u64)>> = Vec::new();
        for slot in slots {
            let held = slot.list.as_ref().and_then(|(hash, position)| {
                let tracks = page.held.get(hash)?;
                Some(usize::try_from(*position).ok().and_then(|at| tracks.get(at)).copied())
            });
            match held {
                Some(track) => (found.push(track), wanted.push(None)),
                None => (found.push(None), wanted.push(slot.list.clone())),
            };
        }
        if wanted.iter().any(Option::is_some) {
            let stored = db.read(move |conn| read_stored(conn, user, &wanted)).await?;
            for (found, stored) in found.iter_mut().zip(stored) {
                *found = found.or(stored);
            }
        }
        Ok(found)
    }

    /// The JSON of `user`'s list `hash`, `{"libraryId", "tracks"}` with IDs as decimal strings,
    /// or `None` if none of their queues holds it: held in memory until saved, else stored. With
    /// `body` false only the list's existence is checked and the JSON is empty.
    pub async fn list_json(
        &self,
        db: &Database,
        user: i64,
        hash: &str,
        body: bool,
    ) -> Result<Option<Vec<u8>>, DbError> {
        let held = self.lock().get(&user).and_then(|account| account.lists.get(hash).cloned());
        let hash = hash.to_owned();
        db.read(move |conn| {
            if let Some(Held { library_id, tracks }) = held {
                let ids = tracks.iter().copied();
                return Ok(Some(if body { json(library_id, ids) } else { Vec::new() }));
            }
            let row = conn
                .prepare_cached(
                    "SELECT library_id, tracks FROM queue_lists WHERE user_id = ?1 AND hash = ?2",
                )?
                .query_row(params![user, hash], |row| {
                    let bytes = if body { row.get::<_, Vec<u8>>(1)? } else { Vec::new() };
                    Ok((row.get::<_, i64>(0)?, bytes))
                })
                .optional()?;
            Ok(row.map(|(library_id, bytes)| {
                let ids = bytes.as_chunks::<8>().0.iter().map(|id| u64::from_le_bytes(*id));
                if body { json(library_id, ids) } else { Vec::new() }
            }))
        })
        .await
    }
}

/// `{"libraryId": "...", "tracks": ["...", ...]}`.
fn json(library_id: i64, tracks: impl ExactSizeIterator<Item = u64>) -> Vec<u8> {
    use std::fmt::Write;
    let mut out = String::with_capacity(32 + tracks.len() * 22);
    let _ = write!(out, r#"{{"libraryId":"{library_id}","tracks":["#);
    for (i, track) in tracks.enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "\"{track}\"");
    }
    out.push_str("]}");
    out.into_bytes()
}

/// For each of `wanted`, the track at that position of `user`'s stored list, or `None` where
/// the list is gone or the position is past its end. Each list is opened once. A database
/// failure is an error, not an unavailable row.
fn read_stored(
    conn: &Connection,
    user: i64,
    wanted: &[Option<(String, u64)>],
) -> rusqlite::Result<Vec<Option<u64>>> {
    let mut opened: HashMap<&str, Option<rusqlite::blob::Blob<'_>>> = HashMap::new();
    let mut found = Vec::with_capacity(wanted.len());
    for want in wanted {
        let Some((hash, position)) = want else {
            found.push(None);
            continue;
        };
        if !opened.contains_key(hash.as_str()) {
            opened.insert(hash, open(conn, user, hash)?);
        }
        let track = opened[hash.as_str()].as_ref().and_then(|blob| {
            let mut bytes = [0u8; 8];
            let at = usize::try_from(*position).ok()?.checked_mul(8)?;
            blob.read_at_exact(&mut bytes, at).ok()?;
            Some(u64::from_le_bytes(bytes))
        });
        found.push(track);
    }
    Ok(found)
}

/// `user`'s stored list `hash`, open for reading, or `None` if there is no such list.
fn open<'c>(
    conn: &'c Connection,
    user: i64,
    hash: &str,
) -> rusqlite::Result<Option<rusqlite::blob::Blob<'c>>> {
    let rowid: Option<i64> = conn
        .prepare_cached("SELECT rowid FROM queue_lists WHERE user_id = ?1 AND hash = ?2")?
        .query_row(params![user, hash], |row| row.get(0))
        .optional()?;
    rowid.map(|rowid| conn.blob_open(MAIN_DB, c"queue_lists", c"tracks", rowid, true)).transpose()
}
