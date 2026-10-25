//! Row builders for tests. Each `new` fills in every column a test has no opinion on, and a test
//! sets the ones it does with struct update syntax, so a schema change touches this file rather
//! than every test that needs a catalog or an account. `new` derives sort keys from its own
//! arguments, so a test that renames a row and depends on its order sets its keys too.

use std::path::Path;
use std::sync::Arc;

use rusqlite::{Connection, Result, params};

use super::{Database, libraries};

mod catalog;
pub mod scale;

pub use catalog::{Album, Artist, Image, Track, album_artist, scattered_key, track_artist};

/// A database in a new temporary directory, which must outlive it.
pub fn open() -> (tempfile::TempDir, Arc<Database>) {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("jewelcase.db")).unwrap();
    (temp, Arc::new(db))
}

/// Adds library `id` called `name`, with its one root at `root`.
pub fn library(conn: &Connection, id: i64, name: &str, root: impl AsRef<Path>) -> Result<()> {
    libraries::create(conn, Some(id), name, &[root.as_ref()], &[]).map(drop)
}

/// Sweeps `db` as the hourly task does, at `now`, to the end. Blocks the calling thread, which
/// must not be in the async runtime.
pub fn sweep(db: &Database, now: i64) {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(super::retention::sweep(db, now));
}

/// Grants `user` access to `library`.
pub fn access(conn: &Connection, user: i64, library: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO library_access (user_id, library_id, granted_at) VALUES (?1, ?2, 0)",
        [user, library],
    )
    .map(drop)
}

/// An account, created at the epoch.
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub role: &'static str,
    pub status: &'static str,
    pub password: String,
}

impl User {
    /// Active account `id` with `role` and no password, named after it, such as `user2`.
    pub fn new(id: i64, role: &'static str) -> User {
        User {
            id,
            username: format!("user{id}"),
            display_name: format!("User {id}"),
            role,
            status: "active",
            password: String::new(),
        }
    }

    /// Adds the account.
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO users (id, username, display_name, role, status, password, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            params![
                self.id,
                self.username,
                self.display_name,
                self.role,
                self.status,
                self.password
            ],
        )
        .map(drop)
    }
}

/// A device: a session of `user`'s, first seen at the epoch.
pub struct Device {
    pub id: i64,
    pub user: i64,
    pub name: &'static str,
    pub kind: &'static str,
    pub platform: Option<&'static str>,
    /// What is stored in place of its token.
    pub token_hash: [u8; 32],
    /// Why it was signed out, as `devices.revoked_reason` stores it; `None` while signed in.
    pub revoked_reason: Option<&'static str>,
    pub last_seen_at: i64,
}

impl Device {
    /// Desktop device `id` of `user`, signed in with the token behind `token_hash` and last seen
    /// at the epoch.
    pub fn new(user: i64, id: i64, token_hash: [u8; 32]) -> Device {
        Device {
            id,
            user,
            name: "Test",
            kind: "desktop",
            platform: None,
            token_hash,
            revoked_reason: None,
            last_seen_at: 0,
        }
    }

    /// Adds the device.
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO devices (id, user_id, name, type, platform, token_hash, revoked_reason, \
             created_at, last_seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)",
            params![
                self.id,
                self.user,
                self.name,
                self.kind,
                self.platform,
                self.token_hash,
                self.revoked_reason,
                self.last_seen_at
            ],
        )
        .map(drop)
    }
}

/// The text SQLite stores for `value`, bound as a parameter, such as a [`super::Text`] variant's
/// name or a [`super::Json`] value's JSON.
pub fn stored(value: impl rusqlite::ToSql) -> String {
    let conn = Connection::open_in_memory().unwrap();
    conn.query_row("SELECT ?1", [value], |row| row.get(0)).unwrap()
}
