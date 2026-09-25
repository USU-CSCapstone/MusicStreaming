//! Streaming PCM decode.

use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Config, Error, ProbeInfo, Registry};

/// A running decode. Read frames with [`PcmStream::read_frames`]; dropping the
/// stream kills the child. Samples are interleaved `f32` at the file's native
/// sample rate and channel count, as reported by the probe.
pub struct PcmStream {
    child: Child,
    stdout: ChildStdout,
    stderr: Option<ChildStderr>,
    registry: Arc<Registry>,
    stalled: Arc<AtomicBool>,
    stall_timeout: std::time::Duration,
    channels: usize,
    buf: Vec<u8>,
    finished: bool,
}

pub(crate) fn decode_pcm(
    config: &Config,
    registry: &Arc<Registry>,
    path: &Path,
    info: &ProbeInfo,
) -> Result<PcmStream, Error> {
    registry.acquire();
    let mut child = Command::new(&config.ffmpeg)
        .args(["-nostdin", "-hide_banner", "-v", "error", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-f",
            "f32le",
            "-c:a",
            "pcm_f32le",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Spawn {
            binary: config.ffmpeg.clone(),
            source: e,
        })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take();
    let stalled = registry.register(child.id());
    Ok(PcmStream {
        child,
        stdout,
        stderr,
        registry: registry.clone(),
        stalled,
        stall_timeout: config.stall_timeout,
        channels: info.channels as usize,
        buf: Vec::new(),
        finished: false,
    })
}

impl PcmStream {
    /// Fill `out` with up to `max_frames` frames (interleaved). Returns the
    /// number of frames read, or 0 at end of stream. Partial trailing frames
    /// are held back until complete.
    pub fn read_frames(&mut self, out: &mut Vec<f32>, max_frames: usize) -> Result<usize, Error> {
        out.clear();
        if self.finished {
            return Ok(0);
        }
        let frame_bytes = self.channels * 4;
        let want = max_frames.max(1) * frame_bytes;
        let mut chunk = vec![0u8; want];
        let n = loop {
            match self.stdout.read(&mut chunk) {
                Ok(n) => break n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::Io(e)),
            }
        };
        if n == 0 {
            self.finished = true;
            return self.finish();
        }
        self.registry.progress(self.child.id());
        self.buf.extend_from_slice(&chunk[..n]);
        let whole = self.buf.len() / frame_bytes * frame_bytes;
        out.reserve(whole / 4);
        for sample in self.buf[..whole].as_chunks::<4>().0 {
            out.push(f32::from_le_bytes([
                sample[0], sample[1], sample[2], sample[3],
            ]));
        }
        self.buf.drain(..whole);
        Ok(whole / frame_bytes)
    }

    /// Wait for the child and turn its exit into a result. Called at EOF.
    fn finish(&mut self) -> Result<usize, Error> {
        let status = self.child.wait()?;
        self.registry.unregister(self.child.id());
        if self.stalled.load(Ordering::SeqCst) {
            return Err(Error::Stalled(self.stall_timeout));
        }
        if !status.success() {
            let mut stderr = String::new();
            if let Some(mut err) = self.stderr.take() {
                let _ = err.read_to_string(&mut stderr);
            }
            return Err(Error::Failed {
                status: status.code(),
                stderr: stderr.trim().to_owned(),
            });
        }
        Ok(0)
    }

    pub fn channels(&self) -> usize {
        self.channels
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
