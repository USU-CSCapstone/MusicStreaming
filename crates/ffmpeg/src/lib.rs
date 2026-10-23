//! ffmpeg as a child process (`design/scanning.md` §11).
//!
//! Never linked as a library: a decoder crash on a corrupt file is a problem
//! record, and a hang is a timeout kill.
//! Every invocation is read-only against the library — no output paths, no
//! `-y`, always `-nostdin`, and the input always opened through `input()`.
//!
//! Background work (analysis and placeholders) runs through [`Ffmpeg`], whose pool kills a child
//! that stalls; the scanner's threads bound how many run at once. Interactive work, which a
//! listener waits on, never joins that pool: [`Config`] builds its commands, and the caller spawns
//! them under its own bound and timeout.
//!
//! The API is synchronous. Callers run it from blocking pools; decoding is
//! stream I/O and the analysis that consumes it is CPU work, neither of which
//! belongs on an async runtime's workers.

mod decode;
mod error;
mod image;
mod input;
mod lock;
mod pooled;
mod priority;
mod probe;
mod registry;
mod stderr;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use jewelcase_core::Format;

pub use decode::PcmStream;
pub use error::Error;
pub use image::Rgba;
use input::input;
pub use priority::lower_thread_priority;
pub use probe::ProbeInfo;
use registry::Registry;

/// Where the binaries are. Tests point them at stand-ins.
#[derive(Debug, Clone)]
pub struct Config {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Default for Config {
    /// The binaries found on `PATH`.
    fn default() -> Self {
        Config { ffmpeg: PathBuf::from("ffmpeg"), ffprobe: PathBuf::from("ffprobe") }
    }
}

impl Config {
    /// The command that scales the image at `path`, or the picture attached to an audio file, to
    /// fit `max_side` pixels and writes it to standard output as JPEG, for serving. Interactive,
    /// so the caller spawns it outside the pool.
    pub fn resize_command(&self, path: &Path, max_side: u32) -> Command {
        image::command(self, path, max_side, image::JPEG)
    }

    // `transcode` is the playback design's to specify; it is interactive too, so it will build a
    // command here. Deliberately absent until that design exists.
}

/// What the installed ffmpeg can do, checked once at startup.
#[derive(Debug, Clone)]
pub struct Capabilities {
    pub version: String,
    /// Formats whose decoder is missing. Empty means every supported format
    /// can be analyzed.
    pub missing: Vec<Format>,
}

/// Handle to the ffmpeg installation and the pool of background children, which a watchdog
/// thread checks for stalls. Clones share the pool. Each call runs one child, so its callers
/// bound how many run at once: the scanner's pool runs one per thread.
#[derive(Clone)]
pub struct Ffmpeg {
    config: Arc<Config>,
    registry: Arc<Registry>,
}

impl Ffmpeg {
    /// Create the handle and start the watchdog thread. Does not touch the
    /// binaries; call [`Ffmpeg::verify`] for that. Fails if the thread can't start.
    pub fn new(config: Config) -> std::io::Result<Ffmpeg> {
        Ok(Ffmpeg { config: Arc::new(config), registry: Registry::start()? })
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
            .map_err(|e| Error::Spawn { binary: self.config.ffmpeg.clone(), source: e })?;
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
            .map_err(|e| Error::Spawn { binary: self.config.ffmpeg.clone(), source: e })?;
        if !out.status.success() {
            return Err(Error::Failed {
                status: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        let table = String::from_utf8_lossy(&out.stdout);
        let decoders = parse_decoders(&table);

        // ffprobe must exist too; analysis setup needs it.
        Command::new(&self.config.ffprobe)
            .args(["-hide_banner", "-version"])
            .output()
            .map_err(|e| Error::Spawn { binary: self.config.ffprobe.clone(), source: e })?;

        let missing = Format::ALL
            .into_iter()
            .filter(|format| !decoders.contains(&format.ffmpeg_decoder()))
            .collect();
        // WAV and AIFF may carry wider PCM; check the rest of the family.
        for pcm in ["pcm_s24le", "pcm_s32le", "pcm_f32le", "pcm_s24be"] {
            if !decoders.contains(&pcm) {
                tracing::warn!(
                    decoder = pcm,
                    "ffmpeg lacks a PCM decoder some WAV/AIFF files need"
                );
            }
        }
        Ok(Capabilities { version, missing })
    }

    /// Read stream properties needed to interpret a raw PCM decode.
    pub fn probe(&self, path: &Path) -> Result<ProbeInfo, Error> {
        probe::probe(&self.config, &self.registry, path)
    }

    /// Start decoding `path` to interleaved 32-bit float PCM in the sample rate
    /// and channel layout `info` reports, which hold for the whole stream. Its child runs until
    /// the stream ends or is dropped.
    pub fn decode_pcm(&self, path: &Path, info: &ProbeInfo) -> Result<PcmStream, Error> {
        decode::decode_pcm(&self.config, &self.registry, path, info)
    }

    /// Decode the image at `path`, or the picture attached to an audio file, scaled to fit
    /// `max_side` pixels. For small previews: the whole image is held in memory.
    pub fn decode_image(&self, path: &Path, max_side: u32) -> Result<Rgba, Error> {
        image::decode_image(&self.config, &self.registry, path, max_side)
    }

    /// Kills every background child of this handle and its clones, and from now on each as it
    /// starts, so the server stops without waiting for a decode to finish. Each fails with
    /// [`Error::Stopped`].
    pub fn stop_background(&self) {
        self.registry.stop();
    }
}

/// The audio decoders named in the table printed by `ffmpeg -decoders`.
///
/// The rows after the `------` separator look like
/// ` A....D flac                 FLAC (Free Lossless Audio Codec)`. Only audio decoders (flag
/// column starting with `A`) are kept.
fn parse_decoders(table: &str) -> Vec<&str> {
    table
        .lines()
        .map(str::trim_start)
        .skip_while(|line| !line.starts_with("------"))
        .skip(1)
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let (flags, name) = (cols.next()?, cols.next()?);
            flags.starts_with('A').then_some(name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_decoder_table() {
        let table = "Decoders:\n V..... = Video\n A..... = Audio\n ------\n V....D h264  H.264\n A....D flac  FLAC\n AF...D alac  ALAC\n";
        assert_eq!(parse_decoders(table), ["flac", "alac"]);
    }
}
