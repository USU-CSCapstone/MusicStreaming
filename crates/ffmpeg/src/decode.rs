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
/// stream kills the child. Samples are interleaved `f32` at the file's native
/// sample rate and channel count, as reported by the probe.
pub struct PcmStream {
    child: Child,
    stdout: ChildStdout,
    /// The thread draining the child's stderr, which yields its last bytes. `None` once
    /// [`PcmStream::finish`] has joined it.
    stderr: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    registry: Arc<Registry>,
    stalled: Arc<AtomicBool>,
    stall_timeout: std::time::Duration,
    channels: usize,
    buf: Vec<u8>,
    finished: bool,
}

/// Start the decode and register the child with the pool. Blocks while the pool is full. Fails
/// if ffmpeg can't be spawned or the thread draining its stderr can't start.
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
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-f", "f32le", "-c:a", "pcm_f32le", "pipe:1"])
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
        stall_timeout: config.stall_timeout,
        channels: info.channels as usize,
        buf: Vec::new(),
        finished: false,
    };
    // Built first so that if the drain thread can't start, dropping `stream` kills the child.
    stream.stderr = Some(tail(stderr)?);
    Ok(stream)
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
            out.push(f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]));
        }
        self.buf.drain(..whole);
        Ok(whole / frame_bytes)
    }

    /// Wait for the child and its stderr drain, and turn its exit into a result. Called at EOF.
    fn finish(&mut self) -> Result<usize, Error> {
        let status = self.child.wait()?;
        self.registry.unregister(self.child.id());
        let drain = self.stderr.take().expect("finish runs once, at EOF");
        let stderr = drain.join().expect("stderr drain panicked");
        if self.stalled.load(Ordering::SeqCst) {
            return Err(Error::Stalled(self.stall_timeout));
        }
        let stderr = stderr?;
        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
            return Err(Error::exited(status, stderr));
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
