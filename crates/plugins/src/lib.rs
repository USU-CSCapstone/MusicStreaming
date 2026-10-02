//! Plugins (`requirements/plugins.md`, `design/plugins.md`): WebAssembly components hosted
//! in-process by Wasmtime, with the manifest each file carries and the permissions an admin
//! grants it.
//!
//! This crate knows nothing of the database or the HTTP API. The server keeps what is
//! installed and granted, and gives a run its library through [`Library`].

mod download;
mod files;
mod host;
pub mod manifest;
mod rules;
pub mod settings;

pub use download::{MAX_SIZE, download};
pub use host::{
    Album, Artist, Event, Grants, Host, Library, Outcome, ScanFinished, Track, TracksChanged,
};
pub use manifest::{InvalidPlugin, Manifest, Permission, PermissionRequest};
