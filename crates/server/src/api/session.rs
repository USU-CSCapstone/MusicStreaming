//! Sessions: the device every login registers, its token, and the `Session` that setup and
//! login answer with (`requirements/users.md` §4).
//!
//! A token is shown once and stored only as its SHA-256 hash (`design/database.md`). It is
//! 256 random bits, so a fast hash is enough; a slow one would only slow every request.

use axum::http::HeaderValue;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::sql::timestamp;
use super::{Code, Id, Problem};
use crate::db::now_ms;

/// The cookie browser clients authenticate with (`cookieAuth` in `api/openapi.yaml`).
pub const COOKIE: &str = "jewelcase_session";

/// Browsers keep a cookie for at most 400 days, so that is how long it asks for.
const COOKIE_MAX_AGE_S: u32 = 400 * 24 * 60 * 60;

/// Longer names and platforms are refused, so no one stores megabytes in a device row.
const MAX_NAME_CHARS: usize = 100;

const DEVICE_TYPES: [&str; 4] = ["phone", "tablet", "desktop", "tv"];

/// The spec's `DeviceRegistration`: how a logging-in device describes itself.
#[derive(Deserialize)]
pub struct DeviceRegistration {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    platform: Option<String>,
}

impl DeviceRegistration {
    /// Refuses an empty or overlong name or platform, or a type the spec does not list.
    pub fn validate(&self) -> Result<(), Problem> {
        if !DEVICE_TYPES.contains(&self.kind.as_str()) {
            return Err(Problem::invalid("device.type must be phone, tablet, desktop, or tv"));
        }
        check_name("device.name", &self.name)?;
        if let Some(platform) = &self.platform {
            check_name("device.platform", platform)?;
        }
        Ok(())
    }
}

/// Refuses a `field` that is blank or longer than [`MAX_NAME_CHARS`].
pub fn check_name(field: &str, value: &str) -> Result<(), Problem> {
    if value.trim().is_empty() || value.chars().count() > MAX_NAME_CHARS {
        return Err(Problem::invalid(format!("{field} must be 1 to {MAX_NAME_CHARS} characters")));
    }
    Ok(())
}

#[derive(Serialize)]
pub struct Session {
    pub token: String,
    user: User,
    device: Device,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct User {
    id: Id,
    username: String,
    display_name: String,
    role: String,
    has_avatar: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    id: Id,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<String>,
    first_seen_at: String,
    last_seen_at: String,
    /// Whether the device holds a realtime connection, which none can open yet.
    connected: bool,
}

/// A new, random token.
pub fn new_token() -> Result<String, Problem> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|error| {
        tracing::error!(%error, "cannot read the system's random source");
        Problem::new(Code::Internal)
    })?;
    Ok(BASE64URL.encode(bytes))
}

/// What the database stores in place of `token`.
pub fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

const SESSION: &str = concat!(
    "SELECT u.id, u.username, u.display_name, u.role, u.avatar_hash IS NOT NULL, ",
    "d.id, d.name, d.type, d.platform, ",
    timestamp!("d.created_at"),
    ", ",
    timestamp!("d.last_seen_at"),
    " FROM devices d JOIN users u ON u.id = d.user_id WHERE d.id = ?1"
);

/// Registers `device` for `user`, signed in with `token`, and returns its session.
pub fn register(
    tx: &Transaction,
    user: i64,
    device: &DeviceRegistration,
    token: String,
) -> rusqlite::Result<Session> {
    let id: i64 = tx.query_row(
        "INSERT INTO devices (id, user_id, name, type, platform, token_hash, created_at, \
         last_seen_at) VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, ?5, ?6, ?6) \
         RETURNING id",
        params![user, device.name, device.kind, device.platform, token_hash(&token), now_ms()],
        |row| row.get(0),
    )?;
    tx.query_row(SESSION, [id], |row| {
        Ok(Session {
            token,
            user: User {
                id: Id(row.get(0)?),
                username: row.get(1)?,
                display_name: row.get(2)?,
                role: row.get(3)?,
                has_avatar: row.get(4)?,
            },
            device: Device {
                id: Id(row.get(5)?),
                name: row.get(6)?,
                kind: row.get(7)?,
                platform: row.get(8)?,
                first_seen_at: row.get(9)?,
                last_seen_at: row.get(10)?,
                connected: false,
            },
        })
    })
}

/// The `Set-Cookie` value that has a browser send `token` with every request to the API at
/// `prefix`, and nowhere else. Scripts cannot read it, and other sites cannot send it.
pub fn cookie(prefix: &str, token: &str) -> HeaderValue {
    let cookie = format!(
        "{COOKIE}={token}; Path={prefix}; Max-Age={COOKIE_MAX_AGE_S}; HttpOnly; SameSite=Strict"
    );
    // The token is base64url and the base path is limited to URL-safe characters (`config`).
    HeaderValue::try_from(cookie).expect("a cookie of header-safe characters")
}
