//! How a plugin run went (`design/plugins.md` §7).

use std::path::PathBuf;

use super::WALL_BUDGET;

/// How a run went, and what it did on the way. A failed run may still have logged, written files
/// the scanner must look at again, or found its service out of reach, which decides whether its
/// failure counts against the plugin, so all of that is kept whichever way it ended.
pub struct Outcome {
    /// The plugin's own summary, or why the run failed.
    pub result: Result<String, RunError>,
    /// What it logged, up to `LOG_LINES` lines.
    pub log: Vec<String>,
    /// How many files it wrote.
    pub written: usize,
    /// Every path it created, replaced, moved, or deleted, for the scanner to look at again.
    pub touched: Vec<PathBuf>,
    /// Whether a request found its service out of reach: held back as `rate-limited`,
    /// `timed-out`, unable to connect, or answered `429` or a `5xx`. A run that failed after one
    /// may have failed only for that, so the server retries it without counting it among the
    /// plugin's own failures.
    pub outage: bool,
}

impl Outcome {
    /// What the admin is shown of how the run went: the plugin's own summary, or why it failed.
    pub fn summary(&self) -> String {
        match &self.result {
            Ok(summary) => summary.clone(),
            Err(error) => error.to_string(),
        }
    }
}

/// Why a run failed.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    /// Its file could not be read or compiled, or not linked against the contract, as the
    /// message says.
    #[error("the plugin could not be loaded: {0}")]
    Unloadable(String),
    /// The host stopped it once it had spent its compute budget.
    #[error("the plugin ran past its compute budget")]
    OutOfCompute,
    /// The host stopped it at the wall-clock limit on handling its event, waiting included.
    #[error("the plugin ran past its {} s time limit", WALL_BUDGET.as_secs())]
    OutOfTime,
    /// It trapped, as the message says: such as at `unreachable`, past a cap on what it may
    /// create, or in a host call that could not go on.
    #[error("the plugin crashed: {0}")]
    Crashed(String),
    /// It answered with an error, in its own words.
    #[error("the plugin reported an error: {0}")]
    Reported(String),
}

impl RunError {
    /// Why the plugin's code stopped with `trap`. The store's epoch callback stops a run past its
    /// compute budget with [`RunError::OutOfCompute`] itself, which wasmtime carries inside the
    /// trap, so a downcast finds it; any other trap is a crash, named by its root cause.
    pub(super) fn trapped(trap: wasmtime::Error) -> RunError {
        let crashed = |trap: wasmtime::Error| RunError::Crashed(trap.root_cause().to_string());
        trap.downcast::<RunError>().unwrap_or_else(crashed)
    }
}
