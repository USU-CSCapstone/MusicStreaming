//! `ffprobe` for the stream facts a raw PCM decode needs.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use serde::Deserialize;

use crate::{Config, Error, Registry};

/// The first audio stream's properties.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeInfo {
    pub codec_name: String,
    pub sample_rate: u32,
    pub channels: u32,
    /// ffmpeg's layout name (`stereo`, `5.1`, ...) when known.
    pub channel_layout: Option<String>,
    pub duration_secs: Option<f64>,
}

impl ProbeInfo {
    /// Frames the decode should yield, when duration is known.
    pub fn estimated_frames(&self) -> Option<u64> {
        self.duration_secs
            .map(|d| (d * self.sample_rate as f64).round() as u64)
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
    codec_name: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    duration: Option<String>,
}

#[derive(Deserialize)]
struct FormatInfo {
    duration: Option<String>,
}

pub(crate) fn probe(
    config: &Config,
    registry: &Arc<Registry>,
    path: &Path,
) -> Result<ProbeInfo, Error> {
    registry.acquire();
    let child = Command::new(&config.ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Spawn {
            binary: config.ffprobe.clone(),
            source: e,
        })?;
    let pid = child.id();
    let stalled = registry.register(pid);
    let out = child.wait_with_output();
    registry.unregister(pid);
    let out = out?;
    if stalled.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(Error::Stalled(config.stall_timeout));
    }
    if !out.status.success() {
        return Err(Error::Failed {
            status: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        });
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
    Ok(ProbeInfo {
        codec_name: stream.codec_name.unwrap_or_default(),
        sample_rate,
        channels,
        channel_layout: stream.channel_layout,
        duration_secs,
    })
}
