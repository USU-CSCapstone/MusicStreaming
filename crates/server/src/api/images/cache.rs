//! Resized artwork, made by ffmpeg once per size and kept in `cache/images`, named by the
//! image's content hash. An image's ID changes whenever its content does, so a cached copy never
//! goes stale.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Semaphore;

/// A resize that takes longer than this is killed: the image is broken or the host is stuck.
const RENDER_TIMEOUT: Duration = Duration::from_secs(30);

/// Where resized images go and how to make them.
pub struct Images {
    cache: PathBuf,
    ffmpeg: PathBuf,
    /// Bounds the resizes running at once. Artwork is an interactive request, so it has its own
    /// bound instead of queueing behind analysis in the ffmpeg pool, which the governor pauses
    /// while listeners play (`design/general.md` §2).
    renders: Semaphore,
    /// Names each resize's temporary file uniquely.
    next_temp: AtomicU64,
}

impl Images {
    pub fn new(cache: PathBuf, ffmpeg: PathBuf) -> Images {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        Images {
            cache,
            ffmpeg,
            renders: Semaphore::new(cores),
            next_temp: AtomicU64::new(0),
        }
    }

    /// The resized copy of the image with content `hash` at `size`, from the file at `source`,
    /// resizing it first if it is not cached yet.
    pub async fn resized(&self, hash: &[u8], size: u32, source: &Path) -> anyhow::Result<PathBuf> {
        let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
        // Split by the first byte, so no one directory holds every image.
        let dir = self.cache.join(&hex[..2]);
        let path = dir.join(format!("{hex}-{size}.jpg"));
        if tokio::fs::try_exists(&path).await? {
            return Ok(path);
        }
        let _permit = self.renders.acquire().await?;
        // Another request may have made it while this one waited.
        if tokio::fs::try_exists(&path).await? {
            return Ok(path);
        }
        let jpeg = self.render(size, source).await?;
        // Written whole and then renamed, so no request ever reads half a file.
        tokio::fs::create_dir_all(&dir).await?;
        let temp = dir.join(format!(
            "{hex}-{size}.{}.tmp",
            self.next_temp.fetch_add(1, Ordering::Relaxed)
        ));
        tokio::fs::write(&temp, jpeg).await?;
        tokio::fs::rename(&temp, &path).await?;
        Ok(path)
    }

    /// Resizes the image at `source` to fit `size`, as JPEG. An embedded image's source is its
    /// track file, and ffmpeg reads the attached picture from that the same way.
    async fn render(&self, size: u32, source: &Path) -> anyhow::Result<Vec<u8>> {
        // `file:` keeps ffmpeg from reading any path as another protocol, and the `min` keeps
        // it from enlarging an image smaller than the size.
        let mut input = std::ffi::OsString::from("file:");
        input.push(source);
        let scale =
            format!("scale='min(iw,{size})':'min(ih,{size})':force_original_aspect_ratio=decrease");
        let child = Command::new(&self.ffmpeg)
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(input)
            .args(["-map", "0:v:0", "-frames:v", "1", "-vf", &scale])
            .args(["-f", "image2pipe", "-c:v", "mjpeg", "-q:v", "3", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // A timed-out render drops the child, and dropping it kills the process.
            .kill_on_drop(true)
            .spawn()?;
        let output = tokio::time::timeout(RENDER_TIMEOUT, child.wait_with_output()).await??;
        if !output.status.success() || output.stdout.is_empty() {
            anyhow::bail!(
                "ffmpeg failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output.stdout)
    }
}
