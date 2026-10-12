//! The caller's devices: `listDevices` (`requirements/users.md` §4). It is also the list Shift
//! sends music to (`requirements/realtime.md` §1), so the account's realtime connections are
//! sent it again whenever it changes.
//!
//! A device logged out with `credentials_changed` keeps its row so it can log back in as
//! itself, but it is not signed in, so it is not listed.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::{Connection, Row};
use serde::Serialize;

use super::authenticate::Caller;
use super::realtime::{Hub, ServerMessage};
use super::sql::timestamp;
use super::{Id, Problem};
use crate::db::Database;

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
    first_seen_at: String,
    last_seen_at: String,
    /// Whether the device holds a realtime connection.
    connected: bool,
    /// Whether this is the calling device. Only `/me` responses say.
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<bool>,
}

#[derive(Serialize)]
pub struct DeviceList {
    items: Vec<Device>,
}

const SELECT: &str = concat!(
    "SELECT id, name, type, platform, ",
    timestamp!("created_at"),
    ", ",
    timestamp!("last_seen_at"),
    " FROM devices"
);

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

/// `user`'s signed-in devices, most recently seen first, none of them `connected` yet.
fn signed_in(conn: &Connection, user: i64) -> rusqlite::Result<Vec<Device>> {
    conn.prepare_cached(&format!(
        "{SELECT} WHERE user_id = ?1 AND token_hash IS NOT NULL ORDER BY last_seen_at DESC, id"
    ))?
    .query_map([user], device)?
    .collect()
}

/// `listDevices`: the caller's signed-in devices, most recently seen first.
pub async fn list(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    caller: Caller,
) -> Result<Json<DeviceList>, Problem> {
    let mut items = db.read(move |conn| signed_in(conn, caller.user)).await?;
    let connected = hub.connected(caller.user);
    for item in &mut items {
        item.current = Some(item.id.0 == caller.device);
        item.connected = connected.contains(&item.id.0);
    }
    Ok(Json(DeviceList { items }))
}

/// Sends `user`'s realtime connections their device list, without `current`, which differs
/// for each. Call it once a change is committed: a device signing in or out, or connecting or
/// disconnecting.
pub fn changed(db: Arc<Database>, hub: Arc<Hub>, user: i64) {
    tokio::spawn(async move {
        // Read in turn, so the list sent last was read after the latest change. A connection
        // that comes or goes during the read sends another list after this one.
        let turn = hub.device_list_turn().await;
        if hub.connected(user).is_empty() {
            return;
        }
        let mut devices = match db.read(move |conn| signed_in(conn, user)).await {
            Ok(devices) => devices,
            Err(error) => return tracing::warn!(%error, "cannot read devices to send"),
        };
        let connected = hub.connected(user);
        for device in &mut devices {
            device.connected = connected.contains(&device.id.0);
        }
        let closed = hub.broadcast(user, &ServerMessage::Devices { devices });
        drop(turn);
        if closed {
            // That list counts the connections it closed for falling behind as connected.
            changed(db, hub, user);
        }
    });
}

#[cfg(test)]
mod tests;
