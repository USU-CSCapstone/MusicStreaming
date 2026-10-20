//! Streaming PCM decode.

use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use crate::stderr::tail;
use crate::{Config, Error, ProbeInfo, Registry, input};

/// A running decode. Read frames with [`PcmStream::read_frames`]; dropping the
/// stream kills the child. Samples are interleaved `f32` in the sample rate and
/// channel layout the probe reported, which hold for the whole stream.
pub struct PcmStream {
    child: Child,
    stdout: ChildStdout,
    /// The thread draining the child's stderr, which yields its last bytes. `None` once
    /// [`PcmStream::finish`] has joined it.
    stderr: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    registry: Arc<Registry>,
    stalled: Arc<AtomicBool>,
    channels: usize,
    /// Read space, reused by every read. Its first `held` bytes are a partial frame left over
    /// from the last read.
    buf: Vec<u8>,
    held: usize,
    finished: bool,
}

/// Start the decode in the shape `info` reports and register the child with the pool. Blocks
/// while the pool is full. Fails if ffmpeg can't be spawned or the thread draining its stderr
/// can't start.
pub(crate) fn decode_pcm(
    config: &Config,
    registry: &Arc<Registry>,
    path: &Path,
    info: &ProbeInfo,
) -> Result<PcmStream, Error> {
    registry.acquire();
    let mut child = Command::new(&config.ffmpeg)
        .args(["-nostdin", "-hide_banner", "-v", "error"])
        .args(input(path))
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn"])
        .args(shape(info))
        .args(["-f", "f32le", "-c:a", "pcm_f32le", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Spawn { binary: config.ffmpeg.clone(), source: e })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stalled = registry.register(child.id());
    let mut stream = PcmStream {
        child,
        stdout,
        stderr: None,
        registry: registry.clone(),
        stalled,
        channels: info.channels as usize,
        buf: Vec::new(),
        held: 0,
        finished: false,
    };
    // Built first so that if the drain thread can't start, dropping `stream` kills the child.
    stream.stderr = Some(tail(stderr)?);
    Ok(stream)
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
    /// stream is dropped.
    pub fn read_frames(&mut self, out: &mut Vec<f32>, max_frames: usize) -> Result<usize, Error> {
        out.clear();
        if self.finished {
            return Ok(0);
        }
        let frame_bytes = self.channels * 4;
        let end = self.held + max_frames.max(1) * frame_bytes;
        if self.buf.len() < end {
            self.buf.resize(end, 0);
        }
        // A pipe may hand over less than a frame, which is not yet a frame to return.
        while self.held < frame_bytes {
            match self.stdout.read(&mut self.buf[self.held..end]) {
                Ok(0) => {
                    self.finished = true;
                    return self.finish();
                }
                Ok(n) => self.held += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Error::Io(e)),
            }
        }
        self.registry.progress(self.child.id());
        let whole = self.held / frame_bytes * frame_bytes;
        let samples = self.buf[..whole].as_chunks::<4>().0;
        out.extend(samples.iter().map(|sample| f32::from_le_bytes(*sample)));
        self.buf.copy_within(whole..self.held, 0);
        self.held -= whole;
        Ok(whole / frame_bytes)
    }

    /// Wait for the child and its stderr drain, and turn its exit into a result. Called at EOF.
    fn finish(&mut self) -> Result<usize, Error> {
        let status = self.child.wait()?;
        self.registry.unregister(self.child.id());
        let drain = self.stderr.take().expect("finish runs once, at EOF");
        let stderr = drain.join().expect("stderr drain panicked");
        if self.stalled.load(Ordering::SeqCst) {
            return Err(Error::Stalled);
        }
        let stderr = stderr?;
        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
            return Err(Error::exited(status, stderr));
        }
        Ok(0)
    }
}

impl Drop for PcmStream {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.registry.unregister(self.child.id());
        }
    }
}
