//! How a plugin run went (`design/plugins.md` §7).

use std::fmt;
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
    /// Every path it created, replaced, moved, or deleted, or set out to, for the scanner to look
    /// at again.
    pub touched: Vec<PathBuf>,
    /// Whether a request found its service out of reach: held back as `rate-limited`,
    /// `timed-out`, unable to connect, or answered `429` or a `5xx`. A run that failed after one
    /// may have failed only for that, so the server retries it without counting it among the
    /// plugin's own failures.
    pub outage: bool,
    /// What the host refused it for want of a permission or an approved destination.
    pub refused: Refused,
    /// The ticks of compute it spent, each about a millisecond of its code running.
    pub ticks: u64,
}

impl Outcome {
    /// A run that failed with `error` before the plugin's code ran.
    pub(super) fn failed(error: RunError) -> Outcome {
        let (log, touched, refused) = (Vec::new(), Vec::new(), Refused::default());
        Outcome { result: Err(error), log, written: 0, touched, outage: false, refused, ticks: 0 }
    }

    /// The plugin's own summary, or why the run failed.
    pub fn summary(&self) -> String {
        match &self.result {
            Ok(summary) => summary.clone(),
            Err(error) => error.to_string(),
        }
    }

    /// What is recorded of the run for the admin: its summary, then what it was refused, if
    /// anything, and about how much compute it spent, in milliseconds under a second and
    /// otherwise in seconds to a tenth, such as `done (refused Network access; about 42 ms of
    /// compute)`.
    pub fn report(&self) -> String {
        let refused = if self.refused.is_empty() {
            String::new()
        } else {
            format!("refused {}; ", self.refused)
        };
        let compute = match self.ticks {
            ticks @ 0..1000 => format!("{ticks} ms"),
            ticks => format!("{}.{} s", ticks / 1000, ticks % 1000 / 100),
        };
        format!("{} ({refused}about {compute} of compute)", self.summary())
    }
}

/// How many distinct refusals a run keeps by name. A plugin can try thousands of hosts, so any
/// past these are only counted.
const REFUSALS_NAMED: usize = 10;
/// The most of a refused name kept, in characters, as a host can be long.
const NAME_CHARS: usize = 200;

/// What a run was refused: each permission it lacked or host it was kept from, named once, in
/// the order it first tried them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Refused {
    /// The first [`REFUSALS_NAMED`] things it was refused, each cut to [`NAME_CHARS`].
    pub named: Vec<String>,
    /// Its refused attempts at anything else.
    pub more: usize,
}

impl Refused {
    /// Records a refusal of `what`: a permission's title or a host.
    pub(super) fn add(&mut self, what: &str) {
        let what: String = what.chars().take(NAME_CHARS).collect();
        if self.named.contains(&what) {
            return;
        }
        if self.named.len() < REFUSALS_NAMED {
            self.named.push(what);
        } else {
            self.more += 1;
        }
    }

    /// Whether nothing was refused.
    pub fn is_empty(&self) -> bool {
        self.named.is_empty()
    }
}

impl fmt::Display for Refused {
    /// The names, then how many more attempts there were, such as `a.example, b.example and 3
    /// more attempts`.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.named.join(", "))?;
        match self.more {
            0 => Ok(()),
            1 => f.write_str(" and 1 more attempt"),
            more => write!(f, " and {more} more attempts"),
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

#[cfg(test)]
mod tests;
