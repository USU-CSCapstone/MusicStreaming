//! Why a lookup did not do what it was asked, and whether the run should stop over it, without
//! any I/O. Tested natively.

use std::fmt;

/// Why a lookup has no answer, in the words the run's log and summary use.
#[derive(Debug, PartialEq)]
pub enum Failure {
    /// No lookup can be answered now: the network was not granted, the request was held back as
    /// `rate-limited`, timed out, or failed, or MusicBrainz answered `429` or a `5xx`. The run
    /// stops with an error, before the track counts as looked up, so it is looked up again later.
    Unavailable(String),
    /// This track's lookup failed, and asking again soon would fail the same way: a refusal,
    /// any other answer, or one it cannot read. The run goes on to the next track.
    Failed(String),
}

impl Failure {
    /// MusicBrainz answered with `status`, which is not its `200` with results: a `429` or a
    /// `5xx`, its answer when asked too often, is worth asking again soon, and any other is not.
    pub fn answered(status: u16) -> Failure {
        let why = format!("MusicBrainz answered {status}");
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
            let why = format!("MusicBrainz answered {status}");
            assert_eq!(Failure::answered(status), Failure::Unavailable(why));
        }
        for status in [301, 400, 401, 403, 410, 600] {
            let why = format!("MusicBrainz answered {status}");
            assert_eq!(Failure::answered(status), Failure::Failed(why));
        }
    }

    #[test]
    fn a_failure_shows_its_message() {
        assert_eq!(Failure::answered(400).to_string(), "MusicBrainz answered 400");
    }
}
