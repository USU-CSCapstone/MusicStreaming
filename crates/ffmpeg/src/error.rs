use std::path::PathBuf;

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
    /// Killed by the watchdog after producing nothing for the stall timeout.
    #[error("ffmpeg produced no output for {0:?} and was killed")]
    Stalled(std::time::Duration),
    #[error("ffprobe output could not be parsed: {0}")]
    Probe(String),
    #[error("no audio stream in file")]
    NoAudioStream,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
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
