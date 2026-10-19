//! Row builders for tests. Each `new` fills in every column a test has no opinion on, and a test
//! sets the ones it does with struct update syntax, so a schema change touches this file rather
//! than every test that needs a catalog or an account. `new` derives sort keys from its own
//! arguments, so a test that renames a row and depends on its order sets its keys too.

use std::path::Path;

use rusqlite::{Connection, Result, params};

use super::libraries;

mod catalog;

pub use catalog::{Album, Artist, Image, Track, scattered_key};

/// Adds library `id` called `name`, with its one root at `root`.
pub fn library(conn: &Connection, id: i64, name: &str, root: impl AsRef<Path>) -> Result<()> {
    libraries::create(conn, Some(id), name, &[root.as_ref()], &[]).map(drop)
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
            "INSERT INTO users (id, username, display_name, role, status, password, created_at, \
             updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0)",
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
    /// What is stored in place of its token; `None` is a device signed out because its
    /// account's credentials changed.
    pub token_hash: Option<[u8; 32]>,
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
            token_hash: Some(token_hash),
            last_seen_at: 0,
        }
    }

    /// Adds the device.
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        let revoked = self.token_hash.is_none().then_some("credentials_changed");
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
                revoked,
                self.last_seen_at
            ],
        )
        .map(drop)
    }
}
