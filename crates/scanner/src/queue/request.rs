//! A request for work, and the saving of a scan taken off the queue as cancelled
//! (`design/scanning.md` §9).

use std::collections::VecDeque;

use super::{Inner, coalesce_scopes};
use crate::clock::now_ms;
use crate::scan::{Scan, ScanId, ScanState, Trigger};
use crate::scope::Scope;
use jewelcase_core::Recover;

impl Inner {
    /// [`Scanner::request`](super::Scanner::request). The queue lock is held while the new scan's
    /// row is written, so concurrent requests for the same work queue it once. The lock is
    /// released before the rows of the scans the new one replaces are saved cancelled, and those
    /// are saved after the new row, so a crash between never leaves their work without a queued
    /// row. Waits on the store for those writes, so it must not be called from an async thread.
    pub(super) fn request(&self, trigger: Trigger, scopes: Vec<Scope>) -> Option<ScanId> {
        let scopes = coalesce_scopes(scopes);
        let mut q = self.queue.lock().recover();
        if let Some(id) = q.covering(&scopes) {
            return Some(id);
        }
        let scan = match self.store.create_scan(self.library.id, trigger, &scopes) {
            Ok(scan) => scan,
            Err(error) => {
                tracing::error!(library = self.library.id, ?trigger, %error, "scan not queued");
                return None;
            }
        };
        let (replaced, kept): (VecDeque<Scan>, VecDeque<Scan>) = std::mem::take(&mut q.queued)
            .into_iter()
            .partition(|queued| scan.covers(&queued.scopes));
        q.queued = kept;
        let id = scan.id;
        q.queued.push_back(scan);
        drop(q);
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
