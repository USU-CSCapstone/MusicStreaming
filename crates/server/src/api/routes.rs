//! The API's routes by area: each module lists the paths under one prefix, in full, and
//! [`super::router`] merges them under the gates they share.

pub mod admin;
pub mod libraries;
pub mod me;
