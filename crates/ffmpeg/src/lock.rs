//! Taking a std lock whether or not it is poisoned.

use std::sync::{LockResult, PoisonError};

/// The guard a lock gave, poisoned or not.
pub(crate) trait Recover<G> {
    /// The guard, even if another thread panicked while it held the lock, so that one panic stays
    /// with its thread instead of failing every later user of the lock. The data is as that
    /// thread left it.
    fn recover(self) -> G;
}

impl<G> Recover<G> for LockResult<G> {
    fn recover(self) -> G {
        self.unwrap_or_else(PoisonError::into_inner)
    }
}
