//! The caller's devices: the registration every login makes, and `listDevices`
//! (`requirements/users.md` §4). That list is also the one Shift sends music to
//! (`requirements/realtime.md` §1), so the account's realtime connections are sent it again
//! whenever it changes.
//!
//! A device signed out by anything but its own `logout` keeps its row and token hash, so its
//! next request is told why. It is not signed in, so it is not listed.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use rusqlite::{Connection, Row, Transaction, params};
use serde::{Deserialize, Serialize};

use super::extract::Path;
use super::me::check_name;
use super::realtime::{Hub, Revoked, ServerMessage};
use super::scope::Caller;
use super::time::serialize_rfc3339;
use super::{Code, Id, Problem};
use crate::db::{Database, new_id, now_ms};

/// The spec's `Device`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    id: Id,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<String>,
    #[serde(serialize_with = "serialize_rfc3339")]
    first_seen_at: i64,
    #[serde(serialize_with = "serialize_rfc3339")]
    last_seen_at: i64,
    /// Whether the device holds a realtime connection.
    connected: bool,
    /// Whether this is the calling device. Only `/me` responses say.
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<bool>,
}

#[derive(Serialize)]
pub struct DeviceList {
    pub(super) items: Vec<Device>,
}

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

const SELECT: &str = "SELECT id, name, type, platform, created_at, last_seen_at FROM devices";

/// Reads a [`SELECT`] row as a device, not yet `connected` and without `current`.
fn device(row: &Row) -> rusqlite::Result<Device> {
    Ok(Device {
        id: Id(row.get(0)?),
        name: row.get(1)?,
        kind: row.get(2)?,
        platform: row.get(3)?,
        first_seen_at: row.get(4)?,
        last_seen_at: row.get(5)?,
        connected: false,
        current: None,
    })
}

/// Device `id`.
pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Device> {
    conn.prepare_cached(&format!("{SELECT} WHERE id = ?1"))?.query_row([id], device)
}

/// Adds `device` as `user`'s, signed in with the token whose hash is `token_hash`, and
/// returns it.
pub fn register(
    tx: &Transaction,
    user: i64,
    device: &DeviceRegistration,
    token_hash: &[u8; 32],
) -> rusqlite::Result<Device> {
    let id = new_id()?;
    tx.execute(
        "INSERT INTO devices (id, user_id, name, type, platform, token_hash, created_at, \
         last_seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![id, user, device.name, device.kind, device.platform, token_hash, now_ms()],
    )?;
    get(tx, id)
}

/// `user`'s signed-in devices, most recently seen first, none of them `connected` yet.
pub(super) fn signed_in(conn: &Connection, user: i64) -> rusqlite::Result<Vec<Device>> {
    conn.prepare_cached(&format!(
        "{SELECT} WHERE user_id = ?1 AND revoked_reason IS NULL ORDER BY last_seen_at DESC, id"
    ))?
    .query_map([user], device)?
    .collect()
}

/// Which of an account's signed-in devices [`sign_out`] ends.
pub enum Which {
    /// This one.
    One(i64),
    /// All of them but `keep`, if given.
    All { keep: Option<i64> },
}

/// Signs out the devices of `user` that `which` names and are still signed in, as `logged_out`,
/// keeping their rows and token hashes so each one's next request is told why. Returns each
/// one's ID and name. Once the write commits, close their connections with `Hub::revoke` or
/// `Hub::revoke_all` and call [`changed`].
pub fn sign_out(
    conn: &Connection,
    user: i64,
    which: Which,
) -> rusqlite::Result<Vec<(i64, String)>> {
    let (only, keep) = match which {
        Which::One(id) => (Some(id), None),
        Which::All { keep } => (None, keep),
    };
    conn.prepare_cached(
        "UPDATE devices SET revoked_reason = 'logged_out' \
         WHERE user_id = ?1 AND revoked_reason IS NULL \
           AND (?2 IS NULL OR id = ?2) AND (?3 IS NULL OR id <> ?3) \
         RETURNING id, name",
    )?
    .query_map(params![user, only, keep], |row| Ok((row.get(0)?, row.get(1)?)))?
    .collect()
}

/// Sets `connected` on each of `user`'s `devices` that holds a realtime connection.
pub(super) fn mark_connected(hub: &Hub, user: i64, devices: &mut [Device]) {
    let connected = hub.connected(user);
    for device in devices {
        device.connected = connected.contains(&device.id.0);
    }
}

/// `listDevices`: the caller's signed-in devices, most recently seen first.
pub async fn list(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    caller: Caller,
) -> Result<Json<DeviceList>, Problem> {
    let mut items = db.read(move |conn| signed_in(conn, caller.user)).await?;
    mark_connected(&hub, caller.user, &mut items);
    for item in &mut items {
        item.current = Some(item.id.0 == caller.device);
    }
    Ok(Json(DeviceList { items }))
}

/// `signOutMyDevice`: signs out another of the caller's signed-in devices, `404` if it is not
/// the caller's or not signed in. The calling device signs out with `logout`.
pub async fn sign_out_mine(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    caller: Caller,
    Path(Id(device)): Path<Id>,
) -> Result<StatusCode, Problem> {
    if device == caller.device {
        return Err(Problem::invalid("This is the device you are using. Log out instead."));
    }
    sign_out_of(db, hub, caller.user, Which::One(device)).await
}

/// `signOutMyOtherDevices`: signs out every signed-in device of the caller but the calling one.
pub async fn sign_out_mine_others(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    caller: Caller,
) -> Result<StatusCode, Problem> {
    sign_out_of(db, hub, caller.user, Which::All { keep: Some(caller.device) }).await
}

/// Signs out `which` of `user`'s devices, then closes their connections and sends the rest the
/// new list.
async fn sign_out_of(
    db: Arc<Database>,
    hub: Arc<Hub>,
    user: i64,
    which: Which,
) -> Result<StatusCode, Problem> {
    let one = matches!(which, Which::One(_));
    // Spawned: a client that hangs up drops this request, which must not leave the connections
    // open once the sign-out is committed.
    let done = tokio::spawn(async move {
        let signed_out = db.write(move |tx| sign_out(tx, user, which)).await?;
        if one && signed_out.is_empty() {
            return Err(Problem::not_found());
        }
        if signed_out.is_empty() {
            return Ok(());
        }
        for (device, _) in signed_out {
            hub.revoke(user, device, Revoked::LoggedOut);
        }
        changed(db, hub, user);
        Ok::<_, Problem>(())
    });
    done.await.map_err(|error| {
        tracing::error!(%error, "signing out devices did not complete");
        Problem::new(Code::Internal)
    })??;
    Ok(StatusCode::NO_CONTENT)
}

/// Sends `user`'s realtime connections their device list, without `current`, which differs
/// for each. Call it once a change is committed: a device signing in or out, or connecting or
/// disconnecting.
pub fn changed(db: Arc<Database>, hub: Arc<Hub>, user: i64) {
    tokio::spawn(async move {
        // Read in turn, so the list sent last was read after the latest change. A connection
        // that comes or goes during the read sends another list after this one, unless one is
        // already waiting to.
        let Some(_turn) = hub.device_list_turn(user).await else { return };
        let connected = hub.connected(user);
        if connected.is_empty() {
            return;
        }
        let mut devices = match db.read(move |conn| signed_in(conn, user)).await {
            Ok(devices) => devices,
            Err(error) => return tracing::warn!(%error, "cannot read devices to send"),
        };
        for device in &mut devices {
            device.connected = connected.contains(&device.id.0);
        }
        hub.broadcast(user, &ServerMessage::Devices { devices });
    });
}

#[cfg(test)]
mod tests;
