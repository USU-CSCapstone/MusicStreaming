//! Stage 8: the one decode pass (`requirements/scanning.md` §1,
//! `design/scanning.md` §12).
//!
//! ffmpeg streams PCM once; two sinks consume every block: the loudness
//! meter and the waveform builder. Adding a result that needs the audio
//! means adding a sink here, never a second decode.

pub mod waveform;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use ebur128::{EbuR128, Mode};
use jewelcase_ffmpeg::{Ffmpeg, ProbeInfo};

use crate::governor::Governor;
use crate::jobs::{Failure, RETRY_AFTER, Retries, RunReport};
use crate::store::{FileToDecode, Store};
use crate::types::*;
use waveform::WaveformBuilder;

/// Bumped whenever any sink's output changes; tracks below it are reprocessed
/// in the background (`requirements/deployment.md` §5).
pub const ANALYZER_VERSION: u32 = 1;

/// Default waveform resolution. The playback design owns the final number.
pub const DEFAULT_WAVEFORM_BINS: usize = 512;

/// Frames per block handed to the sinks.
const BLOCK_FRAMES: usize = 8192;

#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    #[error(transparent)]
    Ffmpeg(#[from] jewelcase_ffmpeg::Error),
    #[error("loudness meter: {0}")]
    Meter(#[from] ebur128::Error),
}

/// Runs analysis for one library.
pub struct Analyzer {
    ffmpeg: Ffmpeg,
    store: Arc<dyn Store>,
    governor: Arc<Governor>,
    waveform_bins: usize,
    retries: Retries,
}

impl Analyzer {
    /// Analyzes with `ffmpeg` into `store`, pausing whenever `governor` does; attaches ffmpeg's
    /// processes to `governor` so a pause stops decodes too.
    pub fn new(ffmpeg: Ffmpeg, store: Arc<dyn Store>, governor: Arc<Governor>) -> Analyzer {
        governor.attach_ffmpeg(ffmpeg.registry().clone());
        Analyzer {
            ffmpeg,
            store,
            governor,
            waveform_bins: DEFAULT_WAVEFORM_BINS,
            retries: Retries::new(RETRY_AFTER),
        }
    }

    /// Skips a track whose file cannot be opened for `retry_after`, rather than an hour.
    pub fn with_retry_after(mut self, retry_after: Duration) -> Analyzer {
        self.retries = Retries::new(retry_after);
        self
    }

    /// Builds waveforms of `bins` peak and RMS bins, at least one, rather than
    /// [`DEFAULT_WAVEFORM_BINS`].
    pub fn with_waveform_bins(mut self, bins: usize) -> Analyzer {
        self.waveform_bins = bins.max(1);
        self
    }

    /// Analyze one file, independent of the store. The unit the tests use.
    pub fn analyze_file(&self, path: &Path) -> Result<AnalysisResult, AnalysisError> {
        let info: ProbeInfo = self.ffmpeg.probe(path)?;
        let mut stream = self.ffmpeg.decode_pcm(path, &info)?;
        let channels = info.channels as usize;

        let mut meter = EbuR128::new(info.channels, info.sample_rate, Mode::I | Mode::TRUE_PEAK)?;
        let mut waveform = WaveformBuilder::new(self.waveform_bins, info.estimated_frames());

        let mut block: Vec<f32> = Vec::with_capacity(BLOCK_FRAMES * channels);
        let mut mono: Vec<f32> = Vec::with_capacity(BLOCK_FRAMES);
        loop {
            self.governor.wait_if_paused();
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
    /// tried again. A failure is handled as [`crate::jobs`] describes.
    pub fn run_once(&self, library: &LibraryId, limit: usize) -> RunReport {
        let fetch = self.retries.fetch(limit);
        let queue = self.store.next_unanalyzed(library, ANALYZER_VERSION, fetch);
        let keep = |track, result: Option<AnalysisResult>| {
            let result = result.unwrap_or_else(failed_result);
            self.store.store_analysis(library, track, result);
        };
        let store = self.store.as_ref();
        self.retries.pass(store, library, queue, limit, |file| self.analyze(file), keep)
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
                seen_at: SystemTime::now(),
            }),
        })
    }

    /// Run in the background until stopped, sleeping when idle or backing off. One track at a
    /// time, so a stop waits for one track at most.
    pub fn start(self: Arc<Self>, library: LibraryId, idle_sleep: Duration) -> AnalysisWorker {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("scanner-analysis".into())
            .spawn(move || {
                while !stop2.load(Ordering::SeqCst) {
                    if self.run_once(&library, 1).idle() {
                        // Woken early by `stop`.
                        std::thread::park_timeout(idle_sleep);
                    }
                }
            })
            .expect("spawn analysis thread");
        AnalysisWorker { stop, thread: Some(thread) }
    }
}

/// An empty result at the current version, so a file judged broken is not decoded on every pass.
fn failed_result() -> AnalysisResult {
    AnalysisResult::empty(ANALYZER_VERSION)
}

pub struct AnalysisWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AnalysisWorker {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
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
