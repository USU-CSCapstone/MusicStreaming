//! Each account's queues and lists, kept in memory and saved in the background
//! (`design/queue.md`, `design/database.md` §6).
//!
//! The server holds the active queue's state and the lists it has not yet saved. A change only
//! marks its account as changed; one task saves every changed account together every second,
//! and once more as the server stops, so the database sees one write however many people are
//! listening.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "starts, restores and deletes have no caller until steps 14 and 19"
    )
)]

mod hash;
mod keep;
mod save;
mod store;
#[cfg(test)]
mod tests;

pub use store::Queues;
