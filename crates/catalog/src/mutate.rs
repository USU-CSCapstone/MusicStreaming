//! The account's writes to the rows a device keeps: like, unlike, hide, unhide, and create, edit,
//! append to and delete a playlist (`design/general.md` §3). The server's handlers and a device
//! both run this code, so a write changes the same rows whichever applies it, as reads already
//! share their SQL. The server adds what is its own: authorization, the feeds, the marks plugins
//! read, idempotency and announcements, each reached through the [`Effect`]s a write reports.
//!
//! A [`Writer`] is one write's view of a database that is in a transaction: it reports each row
//! it changes as an [`Effect`] and, when asked, keeps the [`Undo`] that puts the rows it touched
//! back. [`apply`] is the device's entry: a write by name and JSON arguments, in a transaction of
//! its own, and [`revert`] its undo. A track named by an add is a track ID, already expanded from
//! an album, artist or playlist: expanding needs the whole catalog, which only the server has,
//! so the caller of a device expands on its own side, or sends the references to the server.

use std::collections::HashSet;
use std::fmt;

use jewelcase_core::playlist::Op;
use rusqlite::Connection;
use serde::{Deserialize, Deserializer, Serialize};

use crate::Id;
use crate::sync::{self, Kind, Rows};

mod artwork;
mod edit;
mod hidden;
mod keys;
mod like;
mod playlist;

pub use artwork::{mosaic_id, set as set_artwork};
pub use edit::{
    AppliedOp, Details, Plan, check, check_edit, fresh, plan, plan_for, write, write_edit,
};
pub use like::{Liked, Removed};
pub use playlist::Deleted;

/// How far apart the keys of a playlist's items start, so an item put between two takes the
/// midpoint, 20 times over, before the playlist is renumbered.
pub const KEY_GAP: i64 = 1 << 20;

/// Why a write was refused. Nothing applied.
#[derive(Debug)]
pub enum Error {
    /// The playlist is not the account's own in the library, or the track is not in it.
    NotFound,
    /// The edit was made on another version: the playlist is at this one.
    VersionConflict(i64),
    /// The playlist ID a create chose is used already.
    IdTaken,
    /// The arguments are not acceptable; the message is safe to show.
    Invalid(String),
    Sql(rusqlite::Error),
}

impl Error {
    /// The refusal as JSON for a caller that must not parse messages: `code` (`not_found`,
    /// `version_conflict`, `id_taken` or `invalid`), `message`, and for a conflict
    /// `currentVersion`. A database error is not a refusal: `None`.
    pub fn refusal(&self) -> Option<serde_json::Value> {
        let code = match self {
            Error::NotFound => "not_found",
            Error::VersionConflict(_) => "version_conflict",
            Error::IdTaken => "id_taken",
            Error::Invalid(_) => "invalid",
            Error::Sql(_) => return None,
        };
        let mut refusal = serde_json::json!({ "code": code, "message": self.to_string() });
        if let Error::VersionConflict(version) = self {
            refusal["currentVersion"] = (*version).into();
        }
        Some(refusal)
    }
}

/// The message a caller can show or log.
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound => f.write_str("not found"),
            Error::VersionConflict(version) => write!(f, "version conflict: at {version}"),
            Error::IdTaken => f.write_str("id taken"),
            Error::Invalid(why) => f.write_str(why),
            Error::Sql(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

/// Lets `?` pass a database error on as [`Error::Sql`].
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Error {
        Error::Sql(error)
    }
}

/// Lets `?` pass rows that do not fit the schema on as [`Error::Invalid`].
impl From<sync::Error> for Error {
    fn from(error: sync::Error) -> Error {
        match error {
            sync::Error::Sql(error) => Error::Sql(error),
            error => Error::Invalid(error.to_string()),
        }
    }
}

/// A row a write changed, reported as it is written, in order: the feed row, mark or
/// announcement the server makes of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// A playlist's own row was written: made, edited, its totals moved or its artwork changed.
    Playlist { id: i64 },
    /// A playlist's item was written (`kept`) or deleted.
    Item { playlist: i64, key: i64, kept: bool },
    /// A track was hidden, or unhidden.
    Hidden { track: i64, hidden: bool },
    /// A track entered Liked Songs, or left it.
    Liked { track: i64, liked: bool },
}

/// One write's view of a database that is in a transaction, which the caller commits.
pub struct Writer<'a> {
    conn: &'a Connection,
    user: i64,
    library: i64,
    now: i64,
    journal: Option<Journal>,
    effects: &'a mut dyn FnMut(Effect) -> rusqlite::Result<()>,
}

/// What a write touched, as it was before.
#[derive(Default)]
struct Journal {
    seen: HashSet<(u8, i64, i64)>,
    entries: Vec<Entry>,
}

/// One entity a write touched, with its rows before: none if it was not there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    kind: Kind,
    id: String,
    key: i64,
    before: Option<Rows>,
}

/// The rows a write touched as they were before, in the order it touched them. [`revert`]
/// restores them exactly.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Undo {
    entries: Vec<Entry>,
}

impl<'a> Writer<'a> {
    /// A writer of `user`'s rows in `library` through `conn`, at time `now` (milliseconds), that
    /// calls `effects` for each row it changes. It keeps no undo.
    pub fn new(
        conn: &'a Connection,
        user: i64,
        library: i64,
        now: i64,
        effects: &'a mut dyn FnMut(Effect) -> rusqlite::Result<()>,
    ) -> Writer<'a> {
        Writer { conn, user, library, now, journal: None, effects }
    }

    /// This writer, keeping the [`Undo`] of what it touches.
    fn recording(mut self) -> Writer<'a> {
        self.journal = Some(Journal::default());
        self
    }

    /// The undo of everything written so far, or an empty one for a writer that is not
    /// recording.
    fn undo(&mut self) -> Undo {
        Undo { entries: self.journal.take().map(|journal| journal.entries).unwrap_or_default() }
    }

    /// Reports `effect`.
    fn report(&mut self, effect: Effect) -> rusqlite::Result<()> {
        (self.effects)(effect)
    }

    /// Whether this writer keeps an undo.
    fn is_recording(&self) -> bool {
        self.journal.is_some()
    }

    /// Keeps the rows of the entity as they are now, unless they were kept already: call it
    /// before the first change to them.
    fn touch(&mut self, kind: Kind, id: i64, key: i64) -> rusqlite::Result<()> {
        let Some(journal) = &mut self.journal else { return Ok(()) };
        if !journal.seen.insert((kind as u8, id, key)) {
            return Ok(());
        }
        let text = id.to_string();
        let before = match kind {
            Kind::HiddenTrack => {
                sync::export_for(self.conn, self.user, self.library, kind, &text, key)?
            }
            _ => sync::export(self.conn, self.library, kind, &text, key)?,
        };
        journal.entries.push(Entry { kind, id: text, key, before });
        Ok(())
    }
}

/// Puts back the rows `undo` kept, newest touch first, in one transaction: each entity is as it
/// was before the write, or gone if it was not there. The device's feeds may have written some of
/// the entities since, and a caller that must not undo that leaves it out.
pub fn revert(conn: &Connection, undo: &Undo) -> Result<(), Error> {
    let tx = conn.unchecked_transaction()?;
    for Entry { kind, id, key, before } in undo.entries.iter().rev() {
        match before {
            Some(rows) => sync::apply(&tx, rows)?,
            None => {
                let id = id.parse().map_err(|_| Error::Invalid("an ID is not an ID".to_owned()))?;
                sync::delete(&tx, *kind, id, *key)?;
            }
        }
    }
    Ok(tx.commit()?)
}

/// Tells a `null` from a field left out, as `Some(None)` from `None`.
fn nullable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

/// An edit's op as a device is given it: an add names track IDs.
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum OpArgs {
    Add { items: Vec<Id>, at: Option<usize> },
    Remove { positions: Vec<usize> },
    Move { positions: Vec<usize>, at: usize },
}

/// The arguments of `editPlaylist`, and of `appendToPlaylist` as an add at the end.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditArgs {
    playlist_id: Id,
    /// The version the edit was made on, which an append does not name.
    base_version: Option<i64>,
    title: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    description: Option<Option<String>>,
    pinned: Option<bool>,
    ops: Option<Vec<OpArgs>>,
}

/// The arguments of the writes that name a track.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrackArgs {
    track_id: Id,
    /// The ID a like gives the Liked Songs it makes, when there is none.
    liked_id: Option<Id>,
}

/// The arguments of `createPlaylist`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateArgs {
    id: Id,
    title: String,
    description: Option<String>,
    #[serde(default)]
    items: Vec<Id>,
}

/// The arguments of `appendToPlaylist`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppendArgs {
    playlist_id: Id,
    items: Vec<Id>,
}

/// The arguments of `deletePlaylist`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteArgs {
    playlist_id: Id,
}

/// Reads `args`, JSON text, as `T`.
fn parse<T: serde::de::DeserializeOwned>(args: &str) -> Result<T, Error> {
    serde_json::from_str(args).map_err(|error| Error::Invalid(error.to_string()))
}

/// Applies the write `name` with JSON `args` for `user` in `library` of the device database
/// `conn`, in one transaction, at time `now` (milliseconds), and returns what [`revert`] needs to
/// undo it. The writes are `like` and `unlike` (`trackId`, and for a like `likedId`, the ID to
/// give a Liked Songs it must make), `hide` and `unhide` (`trackId`), `createPlaylist` (`id`,
/// `title`, `description`, `items` as track IDs), `editPlaylist` (`playlistId`, `baseVersion`,
/// `title`, `description`, `pinned`, and `ops` as `editPlaylist` takes them, with track IDs),
/// `appendToPlaylist` (`playlistId`, `items`) and `deletePlaylist` (`playlistId`). Nothing is
/// written when it fails.
pub fn apply(
    conn: &Connection,
    user: i64,
    library: i64,
    name: &str,
    args: &str,
    now: i64,
) -> Result<Undo, Error> {
    let tx = conn.unchecked_transaction()?;
    let mut none = |_| Ok(());
    let mut w = Writer::new(&tx, user, library, now, &mut none).recording();
    match name {
        "like" => {
            let TrackArgs { track_id, liked_id } = parse(args)?;
            w.like(track_id.0, liked_id.map(|id| id.0))?;
        }
        "unlike" => {
            w.take_out(parse::<TrackArgs>(args)?.track_id.0)?;
        }
        "hide" => {
            w.hide(parse::<TrackArgs>(args)?.track_id.0)?;
        }
        "unhide" => {
            w.unhide(parse::<TrackArgs>(args)?.track_id.0)?;
        }
        "createPlaylist" => {
            let CreateArgs { id, title, description, items } = parse(args)?;
            let items = items.into_iter().map(|Id(id)| id).collect();
            w.create_playlist(id.0, &title, description.as_deref(), items)?;
        }
        "editPlaylist" => w.edit_playlist(parse(args)?)?,
        "appendToPlaylist" => {
            let AppendArgs { playlist_id, items } = parse(args)?;
            let add = OpArgs::Add { items, at: None };
            w.edit_playlist(EditArgs {
                playlist_id,
                base_version: None,
                title: None,
                description: None,
                pinned: None,
                ops: Some(vec![add]),
            })?;
        }
        "deletePlaylist" => {
            let DeleteArgs { playlist_id } = parse(args)?;
            w.delete_playlist(playlist_id.0)?.ok_or(Error::NotFound)?;
        }
        other => return Err(Error::Invalid(format!("no write is named {other}"))),
    }
    let undo = w.undo();
    tx.commit()?;
    Ok(undo)
}

impl Writer<'_> {
    /// `editPlaylist` as a device is given it: the checks, the plan and the write the server
    /// makes of the same edit, with the track IDs already expanded.
    fn edit_playlist(&mut self, args: EditArgs) -> Result<(), Error> {
        let EditArgs { playlist_id: Id(id), base_version, title, description, pinned, ops } = args;
        let sizes: Option<Vec<usize>> = ops.as_ref().map(|ops| {
            ops.iter()
                .map(|op| match op {
                    OpArgs::Add { items, .. } => items.len(),
                    OpArgs::Remove { positions } | OpArgs::Move { positions, .. } => {
                        positions.len()
                    }
                })
                .collect()
        });
        let other = description.is_some() || pinned.is_some();
        let title = check_edit(title.as_deref(), other, sizes.as_deref())?;
        let names = title.is_some() || description.is_some();
        let liked = check(self.conn, self.user, self.library, id, base_version, names)?;
        let ops = ops
            .unwrap_or_default()
            .into_iter()
            .map(|op| match op {
                OpArgs::Add { items, at } => {
                    Op::Add { at, items: items.into_iter().map(|Id(id)| id).collect() }
                }
                OpArgs::Remove { positions } => Op::Remove { positions },
                OpArgs::Move { positions, at } => Op::Move { positions, at },
            })
            .collect();
        let plan = plan_for(self.conn, self.library, id, liked, ops, self.now)?;
        write_edit(self, id, base_version, &plan, &Details { title, description, pinned }, liked)
    }
}

#[cfg(test)]
mod tests;
