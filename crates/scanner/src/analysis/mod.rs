//! Stage 8: the one decode pass (`requirements/scanning.md` §1,
//! `design/scanning.md` §12).
//!
//! ffmpeg streams PCM once; two sinks consume every block: the loudness
//! meter and the waveform builder. Adding a result that needs the audio
//! means adding a sink here, never a second decode.

pub mod waveform;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ebur128::{EbuR128, Mode};
use jewelcase_ffmpeg::{Ffmpeg, ProbeInfo};

use crate::clock::now_ms;
use crate::jobs::{Failure, RETRY_AFTER, Retries, RunReport};
use crate::library::LibraryId;
use crate::pool::Job;
use crate::problems::{Problem, ProblemKind};
use crate::store::{AnalysisStore, FileToDecode};
use waveform::WaveformBuilder;

/// Bumped whenever any sink's output changes; tracks below it are reprocessed
/// in the background (`requirements/deployment.md` §5).
pub const ANALYZER_VERSION: u32 = 3;

/// Waveform resolution. The playback design owns the final number.
const WAVEFORM_BINS: usize = 512;

/// Frames per block handed to the sinks.
const BLOCK_FRAMES: usize = 8192;

/// How long the background analysis waits after a pass that backed off or found nothing to do.
const IDLE: Duration = Duration::from_secs(30);

/// Why one file's analysis failed.
#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    #[error(transparent)]
    Ffmpeg(#[from] jewelcase_ffmpeg::Error),
    #[error("loudness meter: {0}")]
    Meter(#[from] ebur128::Error),
}

/// Result of analyzing one track (`design/scanning.md` §12).
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisResult {
    pub analyzer_version: u32,
    /// Integrated loudness, LUFS. `None` for silence.
    pub integrated_lufs: Option<f64>,
    /// True peak across channels, dBTP. `None` for silence.
    pub true_peak_dbtp: Option<f64>,
    /// The RMS level of each waveform bin, on the scale of `jewelcase_core::waveform_level`.
    pub waveform: Vec<u8>,
}

impl AnalysisResult {
    /// A result from analyzer `version` that measured nothing: no loudness, no peak, and no
    /// waveform bins.
    pub fn empty(version: u32) -> AnalysisResult {
        AnalysisResult {
            analyzer_version: version,
            integrated_lufs: None,
            true_peak_dbtp: None,
            waveform: Vec::new(),
        }
    }
}

/// Runs analysis for one library, as a job whose passes may run on several of the pool's
/// threads at once.
pub struct Analyzer {
    ffmpeg: Ffmpeg,
    store: Arc<dyn AnalysisStore>,
    retries: Retries,
}

impl Analyzer {
    /// Analyzes with `ffmpeg` into `store`.
    pub fn new(ffmpeg: Ffmpeg, store: Arc<dyn AnalysisStore>) -> Analyzer {
        Analyzer { ffmpeg, store, retries: Retries::new(RETRY_AFTER) }
    }

    /// Skips a track whose file cannot be opened for `retry_after`, rather than an hour.
    pub fn with_retry_after(mut self, retry_after: Duration) -> Analyzer {
        self.retries = Retries::new(retry_after);
        self
    }

    /// Analyze one file, independent of the store. The unit the tests use.
    pub fn analyze_file(&self, path: &Path) -> Result<AnalysisResult, AnalysisError> {
        let info: ProbeInfo = self.ffmpeg.probe(path)?;
        let mut stream = self.ffmpeg.decode_pcm(path, &info)?;
        let channels = info.channels as usize;

        let mut meter = EbuR128::new(info.channels, info.sample_rate, Mode::I | Mode::TRUE_PEAK)?;
        let mut waveform = WaveformBuilder::new(WAVEFORM_BINS, info.estimated_frames());

        let mut block: Vec<f32> = Vec::with_capacity(BLOCK_FRAMES * channels);
        let mut mono: Vec<f32> = Vec::with_capacity(BLOCK_FRAMES);
        loop {
            let frames = stream.read_frames(&mut block, BLOCK_FRAMES)?;
            if frames == 0 {
                break;
            }
            meter.add_frames_f32(&block)?;
            downmix(&block, channels, &mut mono);
            waveform.push_mono(&mono);
        }

        let integrated = meter.loudness_global().ok().filter(|l| l.is_finite() && *l > -100.0);
        let mut peak: f64 = 0.0;
        for ch in 0..info.channels {
            if let Ok(p) = meter.true_peak(ch) {
                peak = peak.max(p);
            }
        }
        let true_peak_dbtp = if peak > 0.0 { Some(20.0 * peak.log10()) } else { None };

        Ok(AnalysisResult {
            analyzer_version: ANALYZER_VERSION,
            integrated_lufs: integrated,
            true_peak_dbtp,
            waveform: waveform.finish(),
        })
    }

    /// Analyze up to `limit` tracks that need it, skipping those whose files are waiting to be
    /// tried again. A failure is handled as `jobs` describes.
    pub fn run_once(&self, library: LibraryId, limit: usize) -> RunReport {
        let store = self.store.as_ref();
        let next = |fetch| store.next_unanalyzed(library, ANALYZER_VERSION, fetch);
        let keep = |track, result: Option<AnalysisResult>| {
            store.store_analysis(library, track, result.unwrap_or_else(failed_result))
        };
        let decode = |file: &FileToDecode| self.analyze(file);
        let pass = self.retries.pass(store, library, limit, next, decode, keep);
        pass.unwrap_or_else(RunReport::store_failed)
    }

    /// Analyze `file`, if it opens.
    fn analyze(&self, file: &FileToDecode) -> Result<AnalysisResult, Failure> {
        let path = file.open().map_err(Failure::Open)?;
        self.analyze_file(&path).map_err(|error| match error {
            AnalysisError::Ffmpeg(error) => Failure::from_ffmpeg(&file.path, &error),
            AnalysisError::Meter(error) => Failure::Content(Problem {
                path: file.path.clone(),
                kind: ProblemKind::UnsupportedEncoding,
                detail: error.to_string(),
                seen_at: now_ms(),
            }),
        })
    }

    /// The background job that analyzes `library` in the pool, waiting [`IDLE`] when idle or
    /// backing off. One track a pass, so a stop waits for one track at most.
    pub fn job(self: Arc<Self>, library: LibraryId) -> Job {
        Job::new(library, IDLE, move || self.run_once(library, 1))
    }
}

/// An empty result at the current version, so a file judged broken is not decoded on every pass.
fn failed_result() -> AnalysisResult {
    AnalysisResult::empty(ANALYZER_VERSION)
}

/// Average the channels of an interleaved block into `mono`.
fn downmix(interleaved: &[f32], channels: usize, mono: &mut Vec<f32>) {
    mono.clear();
    if channels <= 1 {
        mono.extend_from_slice(interleaved);
        return;
    }
    let scale = 1.0 / channels as f32;
    for frame in interleaved.chunks_exact(channels) {
        mono.push(frame.iter().sum::<f32>() * scale);
    }
}
