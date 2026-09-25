//! ffmpeg as a child process (`design/scanning.md` §11).
//!
//! Never linked as a library: a decoder crash on a corrupt file is a problem
//! record, a hang is a timeout kill, and yielding to listeners is a signal.
//! Every invocation is read-only against the library — no output paths, no
//! `-y`, always `-nostdin`.
//!
//! The API is synchronous. Callers run it from blocking pools; decoding is
//! stream I/O and the analysis that consumes it is CPU work, neither of which
//! belongs on an async runtime's workers.

mod decode;
mod error;
mod probe;
mod registry;

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use jewelcase_core::Format;

pub use decode::PcmStream;
pub use error::Error;
pub use probe::ProbeInfo;
pub use registry::Registry;

/// Where the binaries are and how patient to be with them.
#[derive(Debug, Clone)]
pub struct Config {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    /// A child that produces no output for this long is killed.
    pub stall_timeout: Duration,
    /// Upper bound on concurrently running children (analysis and transcodes
    /// together). The pool is the only place this number lives.
    pub max_processes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: PathBuf::from("ffprobe"),
            stall_timeout: Duration::from_secs(30),
            max_processes: 2,
        }
    }
}

/// What the installed ffmpeg can do, checked once at startup.
#[derive(Debug, Clone)]
pub struct Capabilities {
    pub version: String,
    /// Decoders present, from `ffmpeg -decoders`.
    pub decoders: Vec<String>,
    /// Formats whose decoder is missing. Empty means every supported format
    /// can be analyzed.
    pub missing: Vec<Format>,
}

/// Handle to the ffmpeg installation and the shared process pool.
#[derive(Clone)]
pub struct Ffmpeg {
    config: Arc<Config>,
    registry: Arc<Registry>,
}

impl Ffmpeg {
    /// Create the handle and start the watchdog thread. Does not touch the
    /// binaries; call [`Ffmpeg::verify`] for that.
    pub fn new(config: Config) -> Ffmpeg {
        let registry = Registry::start(config.stall_timeout, config.max_processes);
        Ffmpeg {
            config: Arc::new(config),
            registry,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The shared process registry, for the governor to pause and resume.
    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// Run `ffmpeg -decoders` and check every required decoder is present.
    ///
    /// A missing binary is fatal at startup (`requirements/deployment.md` §1);
    /// a missing decoder is reported once here rather than as scan problems
    /// later.
    pub fn verify(&self) -> Result<Capabilities, Error> {
        let version_out = Command::new(&self.config.ffmpeg)
            .args(["-hide_banner", "-version"])
            .output()
            .map_err(|e| Error::Spawn {
                binary: self.config.ffmpeg.clone(),
                source: e,
            })?;
        let version = String::from_utf8_lossy(&version_out.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim_start_matches("ffmpeg version ")
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();

        let out = Command::new(&self.config.ffmpeg)
            .args(["-hide_banner", "-decoders"])
            .output()
            .map_err(|e| Error::Spawn {
                binary: self.config.ffmpeg.clone(),
                source: e,
            })?;
        if !out.status.success() {
            return Err(Error::Failed {
                status: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        let decoders = parse_decoders(&String::from_utf8_lossy(&out.stdout));

        // ffprobe must exist too; analysis setup needs it.
        Command::new(&self.config.ffprobe)
            .args(["-hide_banner", "-version"])
            .output()
            .map_err(|e| Error::Spawn {
                binary: self.config.ffprobe.clone(),
                source: e,
            })?;

        let mut missing = Vec::new();
        for format in Format::ALL {
            if !decoders.iter().any(|d| d == format.ffmpeg_decoder()) {
                missing.push(format);
            }
        }
        // WAV and AIFF may carry wider PCM; check the rest of the family.
        for pcm in [
            "pcm_s24le",
            "pcm_s32le",
            "pcm_f32le",
            "pcm_s16be",
            "pcm_s24be",
        ] {
            if !decoders.iter().any(|d| d == pcm) {
                tracing::warn!(
                    decoder = pcm,
                    "ffmpeg lacks a PCM decoder some WAV/AIFF files need"
                );
            }
        }
        Ok(Capabilities {
            version,
            decoders,
            missing,
        })
    }

    /// Read stream properties needed to interpret a raw PCM decode.
    pub fn probe(&self, path: &std::path::Path) -> Result<ProbeInfo, Error> {
        probe::probe(&self.config, &self.registry, path)
    }

    /// Start decoding `path` to interleaved 32-bit float PCM at native sample
    /// rate and channel layout. Blocks while the pool is full.
    pub fn decode_pcm(&self, path: &std::path::Path, info: &ProbeInfo) -> Result<PcmStream, Error> {
        decode::decode_pcm(&self.config, &self.registry, path, info)
    }

    // `transcode` is the playback design's to specify; it will share this
    // pool and registry. Deliberately absent until that design exists.
}

/// Parse the decoder table printed by `ffmpeg -decoders`.
///
/// Lines look like ` A....D flac                 FLAC (Free Lossless Audio Codec)`.
/// Only audio decoders (flag column starting with `A`) are kept.
fn parse_decoders(table: &str) -> Vec<String> {
    let mut seen_separator = false;
    let mut out = Vec::new();
    for line in table.lines() {
        let trimmed = line.trim_start();
        if !seen_separator {
            if trimmed.starts_with("------") {
                seen_separator = true;
            }
            continue;
        }
        let mut cols = trimmed.split_whitespace();
        let (Some(flags), Some(name)) = (cols.next(), cols.next()) else {
            continue;
        };
        if flags.starts_with('A') {
            out.push(name.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_decoder_table() {
        let table = "Decoders:\n V..... = Video\n ------\n V....D h264  H.264\n A....D flac  FLAC\n AF...D alac  ALAC\n";
        assert_eq!(parse_decoders(table), vec!["flac", "alac"]);
    }
}
