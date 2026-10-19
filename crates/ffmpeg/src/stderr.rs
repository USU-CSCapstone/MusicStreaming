//! Draining a child's stderr while it runs.

use std::io::{self, Read};
use std::process::ChildStderr;
use std::thread::JoinHandle;

/// How much of the end of stderr is kept for error reports.
const TAIL_BYTES: usize = 4096;

/// Read `stderr` to EOF on its own thread, so a child that writes a lot of it never blocks on a
/// full pipe, and keep at most the last [`TAIL_BYTES`] for reporting why it failed. When the
/// start was cut, the kept text begins at the first whole line. The thread ends when the child
/// closes stderr, which it does by exiting.
pub(crate) fn tail(mut stderr: ChildStderr) -> io::Result<JoinHandle<io::Result<Vec<u8>>>> {
    std::thread::Builder::new().name("ffmpeg-stderr".into()).spawn(move || {
        let mut kept = Vec::with_capacity(2 * TAIL_BYTES);
        let mut chunk = [0u8; TAIL_BYTES];
        let mut cut = false;
        loop {
            let n = match stderr.read(&mut chunk) {
                Ok(0) => {
                    if cut && let Some(newline) = kept.iter().position(|&b| b == b'\n') {
                        kept.drain(..=newline);
                    }
                    return Ok(kept);
                }
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            kept.extend_from_slice(&chunk[..n]);
            let excess = kept.len().saturating_sub(TAIL_BYTES);
            cut |= excess > 0;
            kept.drain(..excess);
        }
    })
}
