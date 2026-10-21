//! Why an import or a lookup did not do what it was asked, and whether the run should stop over
//! it, without any I/O. Tested natively.

use std::fmt;

/// An import's error, as the contract's `error` variant has it (`wit/plugin.wit`), so that what
/// each comes to is checked natively.
#[derive(Debug, PartialEq)]
pub enum ImportError {
    NotGranted,
    Refused(String),
    Failed(String),
    RateLimited(u64),
    TimedOut,
}

impl fmt::Display for ImportError {
    /// The error as the run's log and summary say it, whichever import answered it.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ImportError::NotGranted => f.write_str("a permission it needs was not granted"),
            ImportError::Refused(why) | ImportError::Failed(why) => f.write_str(why),
            ImportError::RateLimited(ms) => {
                write!(f, "the service cannot be asked again for {} s", ms.div_ceil(1000))
            }
            ImportError::TimedOut => f.write_str("the request timed out"),
        }
    }
}

/// Why a lookup has no answer, in the words the run's log and summary use.
#[derive(Debug, PartialEq)]
pub enum Failure {
    /// No lookup can be answered now: the network was not granted, the request was held back as
    /// `rate-limited`, timed out, or failed, or LRCLIB answered `429` or a `5xx`. The run stops
    /// with an error, before the track counts as looked up, so it is looked up again later.
    Unavailable(String),
    /// This track's lookup failed, and asking again soon would fail the same way: a refusal,
    /// any other answer, or one it cannot read. The run goes on to the next track.
    Failed(String),
}

impl From<ImportError> for Failure {
    /// What `http.send`'s error comes to: everything but a refusal may work later, so it stops
    /// the run.
    fn from(error: ImportError) -> Failure {
        match error {
            ImportError::Refused(why) => Failure::Failed(why),
            other => Failure::Unavailable(other.to_string()),
        }
    }
}

impl Failure {
    /// LRCLIB answered with `status`, which is neither a record nor its `404` for none: a `429`
    /// or a `5xx` is worth asking again soon, and any other is not.
    pub fn answered(status: u16) -> Failure {
        let why = format!("LRCLIB answered {status}");
        match status {
            429 | 500..=599 => Failure::Unavailable(why),
            _ => Failure::Failed(why),
        }
    }
}

impl fmt::Display for Failure {
    /// The failure's message, as the run's log and summary say it.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Failure::Unavailable(why) | Failure::Failed(why) => f.write_str(why),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_429_or_a_5xx_answer_stops_the_run() {
        for status in [429, 500, 502, 503, 599] {
            let why = format!("LRCLIB answered {status}");
            assert_eq!(Failure::answered(status), Failure::Unavailable(why));
        }
        for status in [301, 400, 401, 403, 410, 600] {
            let why = format!("LRCLIB answered {status}");
            assert_eq!(Failure::answered(status), Failure::Failed(why));
        }
    }

    #[test]
    fn import_errors_say_why() {
        let said = |error: ImportError| error.to_string();
        assert_eq!(said(ImportError::NotGranted), "a permission it needs was not granted");
        assert_eq!(said(ImportError::Refused("not a destination".into())), "not a destination");
        assert_eq!(said(ImportError::Failed("no such file".into())), "no such file");
        assert_eq!(said(ImportError::TimedOut), "the request timed out");
        assert_eq!(
            said(ImportError::RateLimited(30_000)),
            "the service cannot be asked again for 30 s"
        );
        // Rounded up, so waiting that long is always long enough.
        assert_eq!(said(ImportError::RateLimited(1)), "the service cannot be asked again for 1 s");
    }

    #[test]
    fn only_a_refused_request_lets_the_run_go_on() {
        let refused = ImportError::Refused("not a destination".into());
        assert_eq!(Failure::from(refused), Failure::Failed("not a destination".into()));
        for error in [
            ImportError::NotGranted,
            ImportError::Failed("request failed: connection refused".into()),
            ImportError::RateLimited(30_000),
            ImportError::TimedOut,
        ] {
            let why = error.to_string();
            assert_eq!(Failure::from(error), Failure::Unavailable(why));
        }
    }

    #[test]
    fn a_failure_shows_its_message() {
        assert_eq!(Failure::answered(400).to_string(), "LRCLIB answered 400");
        assert_eq!(Failure::from(ImportError::TimedOut).to_string(), "the request timed out");
    }
}
