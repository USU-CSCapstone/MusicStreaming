//! `ffprobe` for the stream facts a raw PCM decode needs.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use serde::Deserialize;

use crate::{Config, Error, Registry, input};

/// The first audio stream's properties.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeInfo {
    pub sample_rate: u32,
    pub channels: u32,
    /// The channel layout as ffmpeg names it, such as `5.1(side)`, or `None` when the stream
    /// names none and only the channel count is known.
    pub channel_layout: Option<String>,
    pub duration_secs: Option<f64>,
}

impl ProbeInfo {
    /// Frames the decode should yield, when duration is known.
    pub fn estimated_frames(&self) -> Option<u64> {
        self.duration_secs.map(|d| (d * self.sample_rate as f64).round() as u64)
    }
}

#[derive(Deserialize)]
struct Output {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<FormatInfo>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    duration: Option<String>,
}

#[derive(Deserialize)]
struct FormatInfo {
    duration: Option<String>,
}

/// Reads the first audio stream's properties; see [`crate::Ffmpeg::probe`].
pub(crate) fn probe(
    config: &Config,
    registry: &Arc<Registry>,
    path: &Path,
) -> Result<ProbeInfo, Error> {
    registry.acquire();
    let child = Command::new(&config.ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format"])
        .args(input(path))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Spawn { binary: config.ffprobe.clone(), source: e })?;
    let pid = child.id();
    let stalled = registry.register(pid);
    let out = child.wait_with_output();
    registry.unregister(pid);
    let out = out?;
    if stalled.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(Error::Stalled);
    }
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(Error::exited(out.status, stderr));
    }
    let parsed: Output =
        serde_json::from_slice(&out.stdout).map_err(|e| Error::Probe(e.to_string()))?;
    let stream = parsed
        .streams
        .into_iter()
        .find(|s| s.codec_type.as_deref() == Some("audio"))
        .ok_or(Error::NoAudioStream)?;
    let sample_rate = stream
        .sample_rate
        .as_deref()
        .and_then(|s| s.parse().ok())
        .filter(|r: &u32| *r > 0)
        .ok_or_else(|| Error::Probe("missing sample_rate".into()))?;
    let channels = stream
        .channels
        .filter(|c| *c > 0)
        .ok_or_else(|| Error::Probe("missing channels".into()))?;
    let duration_secs = stream
        .duration
        .or(parsed.format.and_then(|f| f.duration))
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d >= 0.0);
    Ok(ProbeInfo { sample_rate, channels, channel_layout: stream.channel_layout, duration_secs })
}
