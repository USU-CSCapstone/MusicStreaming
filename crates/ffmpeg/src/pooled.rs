//! A child process in the pool, from spawn to reap.

use std::io::{self, Read};
use std::process::{Child, ChildStdout, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use rustix::process::Pid;

use crate::stderr::tail;
use crate::{Error, Registry};

/// A child registered with the pool while it runs, its standard error drained on another
/// thread. Dropping it before [`Pooled::finish`] kills and reaps it.
///
/// The child always leaves the pool before it is reaped: once reaped, its pid may be given to
/// another process, which the pool's signals must never reach.
pub(crate) struct Pooled {
    /// The child, holding its standard output.
    child: Child,
    pid: Pid,
    /// The thread draining standard error, which yields its last bytes. `None` only while
    /// [`Pooled::spawn`] starts it, and once [`Pooled::finish`] has joined it.
    drain: Option<JoinHandle<io::Result<Vec<u8>>>>,
    registry: Arc<Registry>,
    stalled: Arc<AtomicBool>,
    reaped: bool,
}

impl Pooled {
    /// Spawn `command`, register the child and start draining its standard error. `command` must
    /// pipe its standard output and standard error. Fails if the child can't be spawned, its
    /// standard error isn't piped, or the drain thread can't start.
    pub(crate) fn spawn(registry: &Arc<Registry>, command: &mut Command) -> Result<Pooled, Error> {
        let child = command
            .spawn()
            .map_err(|source| Error::Spawn { binary: command.get_program().into(), source })?;
        let pid = Pid::from_child(&child);
        let stalled = registry.register(pid);
        let mut pooled =
            Pooled { child, pid, drain: None, registry: registry.clone(), stalled, reaped: false };
        // Built first so that if standard error is not piped or its drain can't start, dropping
        // `pooled` kills the child.
        let stderr = pooled.child.stderr.take().ok_or_else(|| unpiped("standard error"))?;
        pooled.drain = Some(tail(stderr)?);
        Ok(pooled)
    }

    /// The child's standard output. Fails if its command did not pipe it.
    pub(crate) fn stdout(&mut self) -> io::Result<&mut ChildStdout> {
        self.child.stdout.as_mut().ok_or_else(|| unpiped("standard output"))
    }

    /// Restart the child's stall clock.
    pub(crate) fn progress(&self) {
        self.registry.progress(self.pid);
    }

    /// Whether [`Pooled::finish`] has run.
    pub(crate) fn finished(&self) -> bool {
        self.reaped
    }

    /// Unregister the child, reap it and join its stderr drain. Call it when its standard output
    /// has ended. Fails with [`Error::Stalled`] if the watchdog killed the child. If it ended
    /// unsuccessfully, fails with [`Error::Stopped`] once the server is stopping, and otherwise
    /// with the end of what it wrote to standard error. A later call is harmless: it answers from
    /// the same exit, with no standard error left to read.
    pub(crate) fn finish(&mut self) -> Result<(), Error> {
        self.registry.unregister(self.pid);
        self.reaped = true;
        let status = self.child.wait();
        // The drain is gone only once this has run, and there is nothing more to read.
        let stderr = self.drain.take().map_or(Ok(Vec::new()), |drain| {
            drain.join().unwrap_or_else(|_| Err(io::Error::other("ffmpeg's stderr drain panicked")))
        });
        let status = status?;
        if self.stalled.load(Ordering::SeqCst) {
            return Err(Error::Stalled);
        }
        let stderr = stderr?;
        if !status.success() {
            if self.registry.stopping() {
                return Err(Error::Stopped);
            }
            let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
            return Err(Error::exited(status, stderr));
        }
        Ok(())
    }

    /// Run the child to its end and return its standard output, read whole. Fails if the output
    /// can't be read, or as [`Pooled::finish`] does.
    pub(crate) fn output(mut self) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        self.stdout()?.read_to_end(&mut out)?;
        self.finish()?;
        Ok(out)
    }
}

impl Drop for Pooled {
    /// Kill, unregister and reap a child that was not reaped, logging what fails.
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        if let Err(error) = self.child.kill() {
            tracing::warn!(pid = self.pid.as_raw_pid(), %error, "could not kill ffmpeg");
        }
        self.registry.unregister(self.pid);
        if let Err(error) = self.child.wait() {
            tracing::warn!(pid = self.pid.as_raw_pid(), %error, "could not reap ffmpeg");
        }
    }
}

/// The error for a child whose `stream` the command didn't pipe.
fn unpiped(stream: &str) -> io::Error {
    io::Error::other(format!("ffmpeg's {stream} was not piped"))
}

#[cfg(test)]
mod tests;
