//! A request for work, and the saving of a scan taken off the queue as cancelled
//! (`design/scanning.md` §9).

use std::collections::VecDeque;
use std::time::Instant;

use super::{Inner, coalesce_scopes};
use crate::clock::now_ms;
use crate::scan::{Scan, ScanId, ScanState, Trigger};
use crate::scope::Scope;
use jewelcase_core::Recover;

impl Inner {
    /// [`Scanner::request`](super::Scanner::request). The new scan's row is written before the
    /// rows of the scans it replaces are saved cancelled, so a crash between never leaves their
    /// work without a queued row.
    pub(super) fn request(&self, trigger: Trigger, scopes: Vec<Scope>) -> Option<ScanId> {
        let scopes = coalesce_scopes(scopes);
        if let Some(id) = self.queue.lock().recover().covering(&scopes) {
            return Some(id);
        }
        let library = self.library.id;
        let scan = match self.store.create_scan(library, trigger, &scopes) {
            Ok(scan) => scan,
            Err(error) => {
                tracing::error!(
                    library, ?trigger, %error,
                    "scan not queued; retrying after a wait"
                );
                self.queue.lock().recover().retry.add(trigger, scopes, Instant::now());
                // The worker may be waiting with no time limit, and must wait for the retry's.
                self.wake.notify_all();
                return None;
            }
        };
        let (id, replaced) = {
            let mut q = self.queue.lock().recover();
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

    /// Saves `scan`, already taken off the queue, as cancelled. A row that cannot be saved is
    /// logged and stays queued in the store, so the next start runs the scan.
    fn cancelled(&self, scan: &Scan) {
        let ended =
            Scan { state: ScanState::Cancelled, finished_at: Some(now_ms()), ..scan.clone() };
        if let Err(error) = self.store.update_scan(&ended) {
            tracing::error!(scan = scan.id, %error, "cancelled scan not saved");
        }
    }
}
