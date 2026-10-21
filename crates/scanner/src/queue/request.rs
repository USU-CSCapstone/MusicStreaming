//! A request for work, and the saving of a scan taken off the queue as cancelled
//! (`design/scanning.md` §9).

use std::collections::VecDeque;
use std::time::{Instant, SystemTime};

use super::{Inner, coalesce_scopes};
use crate::types::*;

impl Inner {
    /// [`Scanner::request`](super::Scanner::request). The new scan's row is written before the
    /// rows of the scans it replaces are saved cancelled, so a crash between never leaves their
    /// work without a queued row.
    pub(super) fn request(&self, trigger: Trigger, scopes: Vec<Scope>) -> Option<ScanId> {
        let scopes = coalesce_scopes(scopes);
        if let Some(id) = self.queue.lock().unwrap().covering(&scopes) {
            return Some(id);
        }
        let library = self.library.read().unwrap().id;
        let mut scan = Scan {
            id: 0,
            library,
            trigger,
            scopes,
            state: ScanState::Queued,
            started_at: None,
            finished_at: None,
            progress: ScanProgress::default(),
            cursor: None,
        };
        match self.store.create_scan(&scan) {
            Ok(id) => scan.id = id,
            Err(error) => {
                tracing::error!(
                    library, ?trigger, %error,
                    "scan not queued; retrying after a wait"
                );
                self.queue.lock().unwrap().retry.add(trigger, scan.scopes, Instant::now());
                // The worker may be waiting with no time limit, and must wait for the retry's.
                self.wake.notify_all();
                return None;
            }
        }
        let (id, replaced) = {
            let mut q = self.queue.lock().unwrap();
            match q.covering(&scan.scopes) {
                // A request that raced this one queued a scan covering it while this one's row
                // was written, so that row is saved cancelled.
                Some(id) => (id, VecDeque::from([scan])),
                None => {
                    let (replaced, kept) = std::mem::take(&mut q.queued)
                        .into_iter()
                        .partition(|queued| scan.covers(&queued.scopes));
                    q.queued = kept;
                    let id = scan.id;
                    q.queued.push_back(scan);
                    (id, replaced)
                }
            }
        };
        self.wake.notify_all();
        for scan in &replaced {
            self.cancelled(scan);
        }
        Some(id)
    }

    /// Saves `scan`, already taken off the queue, as cancelled, returning whether the store
    /// could. A row that cannot be saved is logged and stays queued in the store, so the next
    /// start runs the scan even if the caller drops it.
    pub(super) fn cancelled(&self, scan: &Scan) -> bool {
        let ended = Scan {
            state: ScanState::Cancelled,
            finished_at: Some(SystemTime::now()),
            ..scan.clone()
        };
        let saved = self.store.update_scan(&ended);
        if let Err(error) = &saved {
            tracing::error!(scan = scan.id, %error, "cancelled scan not saved");
        }
        saved.is_ok()
    }
}
