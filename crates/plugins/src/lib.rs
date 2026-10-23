//! Plugins (`requirements/plugins.md`, `design/plugins.md`): WebAssembly components hosted
//! in-process by Wasmtime, with the manifest each file carries and the permissions an admin
//! grants it.
//!
//! This crate knows nothing of the database or the HTTP API. The server keeps what is
//! installed and granted, and gives a run its library through [`Library`].

mod download;
mod files;
mod host;
mod lock;
pub mod manifest;
mod network;
mod rules;
pub mod settings;
#[cfg(feature = "testing")]
pub mod testing;

pub use download::{MAX_SIZE, download};
pub use host::{
    Album, Artist, Changes, Event, Found, Grants, Host, Library, Outcome, Play, PlayEnd, Playing,
    ResultKind, RunError, ScanFinished, Search, Selected, Track,
};
pub use manifest::{Hook, InvalidPlugin, Manifest, Per, Permission, PermissionRequest, RateLimit};
pub use rules::reaches;
