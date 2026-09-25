//! Stage 8: the one decode pass (`requirements/scanning.md` §1,
//! `design/scanning.md` §12).
//!
//! ffmpeg streams PCM once; three sinks consume every block: the loudness
//! meter, the waveform builder, and the feature extractor. Adding a result
//! that needs the audio means adding a sink here, never a second decode.

pub mod waveform;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use ebur128::{EbuR128, Mode};
use jewelcase_ffmpeg::{Ffmpeg, ProbeInfo};

use crate::governor::Governor;
use crate::problems;
use crate::store::Store;
use crate::types::*;
use waveform::WaveformBuilder;

/// Bumped whenever any sink's output changes; tracks below it are reprocessed
/// in the background (`requirements/deployment.md` §5).
pub const ANALYZER_VERSION: u32 = 1;

/// Default waveform resolution. The playback design owns the final number.
pub const DEFAULT_WAVEFORM_BINS: usize = 512;

/// Frames per block handed to the sinks.
const BLOCK_FRAMES: usize = 8192;

/// The recommendations design's extractor plugs in here. It receives the
/// mono downmix of every block, in order, and returns an opaque descriptor.
pub trait FeatureExtractor: Send + Sync {
    fn begin(&self, sample_rate: u32) -> Box<dyn FeatureSink>;
}

pub trait FeatureSink: Send {
    fn push_mono(&mut self, mono: &[f32]);
    fn finish(self: Box<Self>) -> Option<Vec<u8>>;
}

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
    features: Option<Arc<dyn FeatureExtractor>>,
    waveform_bins: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub analyzed: usize,
    pub failed: usize,
}

impl Analyzer {
    pub fn new(ffmpeg: Ffmpeg, store: Arc<dyn Store>, governor: Arc<Governor>) -> Analyzer {
        governor.attach_ffmpeg(ffmpeg.registry().clone());
        Analyzer {
            ffmpeg,
            store,
            governor,
            features: None,
            waveform_bins: DEFAULT_WAVEFORM_BINS,
        }
    }

    pub fn with_features(mut self, extractor: Arc<dyn FeatureExtractor>) -> Analyzer {
        self.features = Some(extractor);
        self
    }

    pub fn with_waveform_bins(mut self, bins: usize) -> Analyzer {
        self.waveform_bins = bins.max(1);
        self
    }

    /// Analyze one file, independent of the store. The unit the tests use.
    pub fn analyze_file(&self, path: &Path) -> Result<AnalysisResult, AnalysisError> {
        let info: ProbeInfo = self.ffmpeg.probe(path)?;
        let mut stream = self.ffmpeg.decode_pcm(path, &info)?;
        let channels = info.channels as usize;

        let mut meter = EbuR128::new(
            info.channels,
            info.sample_rate,
            Mode::I | Mode::LRA | Mode::TRUE_PEAK,
        )?;
        let mut waveform = WaveformBuilder::new(self.waveform_bins, info.estimated_frames());
        let mut features = self.features.as_ref().map(|f| f.begin(info.sample_rate));

        let mut block: Vec<f32> = Vec::with_capacity(BLOCK_FRAMES * channels);
        let mut mono: Vec<f32> = Vec::with_capacity(BLOCK_FRAMES);
        let mut total_frames: u64 = 0;
        loop {
            self.governor.wait_if_paused();
            let frames = stream.read_frames(&mut block, BLOCK_FRAMES)?;
            if frames == 0 {
                break;
            }
            total_frames += frames as u64;
            meter.add_frames_f32(&block)?;
            downmix(&block, channels, &mut mono);
            waveform.push_mono(&mono);
            if let Some(f) = features.as_mut() {
                f.push_mono(&mono);
            }
        }

        let integrated = meter
            .loudness_global()
            .ok()
            .filter(|l| l.is_finite() && *l > -100.0);
        let range = meter.loudness_range().ok().filter(|l| l.is_finite());
        let mut peak: f64 = 0.0;
        for ch in 0..info.channels {
            if let Ok(p) = meter.true_peak(ch) {
                peak = peak.max(p);
            }
        }
        let true_peak_dbtp = if peak > 0.0 {
            Some(20.0 * peak.log10())
        } else {
            None
        };
        let _ = total_frames;

        Ok(AnalysisResult {
            analyzer_version: ANALYZER_VERSION,
            integrated_lufs: integrated,
            loudness_range_lu: range,
            true_peak_dbtp,
            waveform: waveform.finish(),
            features: features.and_then(|f| f.finish()),
        })
    }

    /// Analyze up to `limit` tracks that need it. Returns what happened;
    /// zero analyzed and zero failed means the queue is empty.
    pub fn run_once(&self, library: &LibraryId, limit: usize) -> RunReport {
        let mut report = RunReport::default();
        for (track_id, path) in self.store.next_unanalyzed(library, ANALYZER_VERSION, limit) {
            match self.analyze_file(&path) {
                Ok(result) => {
                    self.store.store_analysis(library, track_id, result);
                    report.analyzed += 1;
                }
                Err(AnalysisError::Ffmpeg(e)) => {
                    let problem = problems::from_ffmpeg(&path, &e);
                    tracing::warn!(path = %path.display(), kind = ?problem.kind, "analysis failed: {}", problem.detail);
                    // Record a placeholder so the track is not retried every
                    // pass; a later scan of the file clears it by re-upserting.
                    self.store
                        .store_analysis(library, track_id, failed_placeholder());
                    report.failed += 1;
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "analysis failed");
                    self.store
                        .store_analysis(library, track_id, failed_placeholder());
                    report.failed += 1;
                }
            }
        }
        report
    }

    /// Run in the background until stopped, sleeping when idle.
    pub fn start(self: Arc<Self>, library: LibraryId, idle_sleep: Duration) -> AnalysisWorker {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("scanner-analysis".into())
            .spawn(move || {
                while !stop2.load(Ordering::SeqCst) {
                    let report = self.run_once(&library, 8);
                    if report.analyzed == 0 && report.failed == 0 {
                        std::thread::sleep(idle_sleep);
                    }
                }
            })
            .expect("spawn analysis thread");
        AnalysisWorker {
            stop,
            thread: Some(thread),
        }
    }
}

/// An empty result at the current version, so a permanently broken file is
/// not decoded on every pass.
fn failed_placeholder() -> AnalysisResult {
    AnalysisResult {
        analyzer_version: ANALYZER_VERSION,
        integrated_lufs: None,
        loudness_range_lu: None,
        true_peak_dbtp: None,
        waveform: Waveform {
            peaks: Vec::new(),
            rms: Vec::new(),
        },
        features: None,
    }
}

pub struct AnalysisWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AnalysisWorker {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
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
