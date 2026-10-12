//! The caller's devices: `listDevices` (`requirements/users.md` §4). It is also the list Shift
//! sends music to (`requirements/realtime.md` §1).
//!
//! A device logged out with `credentials_changed` keeps its row so it can log back in as
//! itself, but it is not signed in, so it is not listed.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use rusqlite::{Connection, Row};
use serde::Serialize;

use super::authenticate::Caller;
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
    /// Whether the device holds a realtime connection, which none can open yet.
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

/// `listDevices`: the caller's signed-in devices, most recently seen first.
pub async fn list(
    State(db): State<Arc<Database>>,
    caller: Caller,
) -> Result<Json<DeviceList>, Problem> {
    let mut items = db
        .read(move |conn| {
            conn.prepare_cached(&format!(
                "{SELECT} WHERE user_id = ?1 AND token_hash IS NOT NULL \
                 ORDER BY last_seen_at DESC, id"
            ))?
            .query_map([caller.user], device)?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;
    for item in &mut items {
        item.current = Some(item.id.0 == caller.device);
    }
    Ok(Json(DeviceList { items }))
}

#[cfg(test)]
mod tests;
