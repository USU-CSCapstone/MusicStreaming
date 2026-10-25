use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::ExitStatus;

use crate::registry::STALL_TIMEOUT;

/// Why an ffmpeg invocation failed. Variants map onto scan problem kinds in
/// the scanner (`design/scanning.md` §11).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not start {binary}: {source}")]
    Spawn {
        binary: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The process exited nonzero. `stderr` is what it said.
    #[error("ffmpeg exited with status {status:?}: {stderr}")]
    Failed { status: Option<i32>, stderr: String },
    /// The process was killed by a signal: from outside, or by a crash.
    #[error("ffmpeg was killed by signal {signal}: {stderr}")]
    Killed { signal: i32, stderr: String },
    /// Killed by the watchdog: a PCM decode that produced nothing for `STALL_TIMEOUT`, or a
    /// probe or image decode that ran that long.
    #[error("ffmpeg stalled for {STALL_TIMEOUT:?} and was killed")]
    Stalled,
    /// Killed as the server stops ([`crate::Ffmpeg::stop_background`]).
    #[error("ffmpeg was stopped with the server")]
    Stopped,
    #[error("ffprobe output could not be parsed: {0}")]
    Probe(String),
    #[error("image output could not be parsed: {0}")]
    Image(String),
    #[error("no audio stream in file")]
    NoAudioStream,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// The error for a process that ended unsuccessfully with `status`, having said `stderr`.
    pub(crate) fn exited(status: ExitStatus, stderr: String) -> Error {
        match status.signal() {
            Some(signal) => Error::Killed { signal, stderr },
            None => Error::Failed { status: status.code(), stderr },
        }
    }

    /// Whether stderr indicates a decoder is missing rather than a broken file.
    pub fn is_unsupported_encoding(&self) -> bool {
        match self {
            Error::Failed { stderr, .. } => {
                let s = stderr.to_ascii_lowercase();
                s.contains("decoder not found")
                    || s.contains("unknown decoder")
                    || s.contains("codec not currently supported")
                    || s.contains("unsupported codec")
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_is_kept_apart_from_an_exit_code() {
        // Raw wait statuses: a kill by SIGKILL, and an exit with code 1.
        let killed = Error::exited(ExitStatus::from_raw(9), String::new());
        assert!(matches!(killed, Error::Killed { signal: 9, .. }), "{killed:?}");
        assert!(killed.to_string().starts_with("ffmpeg was killed by signal 9"));
        let failed = Error::exited(ExitStatus::from_raw(1 << 8), String::new());
        assert!(matches!(failed, Error::Failed { status: Some(1), .. }), "{failed:?}");
    }
}
