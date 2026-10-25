//! Streaming PCM decode.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use crate::pooled::Pooled;
use crate::{Config, Error, ProbeInfo, Registry, input};

/// A running decode. Read frames with [`PcmStream::read_frames`]; dropping the
/// stream kills the child. Samples are interleaved `f32` in the sample rate and
/// channel layout the probe reported, which hold for the whole stream.
pub struct PcmStream {
    child: Pooled,
    channels: usize,
    /// Read space, reused by every read. Its first `held` bytes are a partial frame left over
    /// from the last read.
    buf: Vec<u8>,
    held: usize,
}

/// Start the decode in the shape `info` reports and register the child with the pool. Fails if
/// ffmpeg can't be spawned or the thread draining its stderr can't start.
pub(crate) fn decode_pcm(
    config: &Config,
    registry: &Arc<Registry>,
    path: &Path,
    info: &ProbeInfo,
) -> Result<PcmStream, Error> {
    let child = Pooled::spawn(
        registry,
        Command::new(&config.ffmpeg)
            .args(["-nostdin", "-hide_banner", "-v", "error"])
            .args(input(path))
            .args(["-map", "0:a:0", "-vn", "-sn", "-dn"])
            .args(shape(info))
            .args(["-f", "f32le", "-c:a", "pcm_f32le", "pipe:1"])
            .stdin(Stdio::null()),
    )?;
    Ok(PcmStream { child, channels: info.channels as usize, buf: Vec::new(), held: 0 })
}

/// The output options that hold the decode to the shape `info` reports: its sample rate, and its
/// channel layout, or its channel count when it names no layout. ffmpeg converts any part of the
/// stream in another shape, so the samples always match what the probe said.
///
/// The layout is named because ffmpeg reads a bare count as its default layout for that count,
/// and would remix a layout like quad or 6.0 into it.
fn shape(info: &ProbeInfo) -> [String; 4] {
    let (option, channels) = match &info.channel_layout {
        Some(layout) => ("-ch_layout", layout.clone()),
        None => ("-ac", info.channels.to_string()),
    };
    [option.into(), channels, "-ar".into(), info.sample_rate.to_string()]
}

impl PcmStream {
    /// Fill `out` with up to `max_frames` frames (interleaved). Returns the
    /// number of frames read, at least one, or 0 at end of stream. Partial
    /// trailing frames are held back until complete, and one left at end of
    /// stream is dropped. At end of stream it fails instead if ffmpeg failed
    /// or the watchdog killed it as stalled.
    pub fn read_frames(&mut self, out: &mut Vec<f32>, max_frames: usize) -> Result<usize, Error> {
        out.clear();
        if self.child.finished() {
            return Ok(0);
        }
        let frame_bytes = self.channels * 4;
        let end = self.held + max_frames.max(1) * frame_bytes;
        if self.buf.len() < end {
            self.buf.resize(end, 0);
        }
        // A pipe may hand over less than a frame, which is not yet a frame to return.
        while self.held < frame_bytes {
            match self.child.stdout().read(&mut self.buf[self.held..end]) {
                Ok(0) => return self.child.finish().map(|()| 0),
                Ok(n) => self.held += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Error::Io(e)),
            }
        }
        self.child.progress();
        let whole = self.held / frame_bytes * frame_bytes;
        let samples = self.buf[..whole].as_chunks::<4>().0;
        out.extend(samples.iter().map(|sample| f32::from_le_bytes(*sample)));
        self.buf.copy_within(whole..self.held, 0);
        self.held -= whole;
        Ok(whole / frame_bytes)
    }
}
