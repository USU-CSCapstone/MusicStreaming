//! Resized artwork, made by ffmpeg once per size and kept in `cache/images`, in a folder for
//! each library and named by the image's content hash. Two libraries holding the same artwork
//! each keep their own copy, so how fast one answers says nothing of what the other has viewed
//! (`requirements/general.md` §3.6). An image's ID changes whenever its content does, so a cached
//! copy never goes stale. An image ffmpeg cannot decode leaves a `.failed` marker instead, so it
//! is not tried again on every request until the next start, when [`Images::sweep`] deletes
//! every marker along with what no image needs any more; changed content has a new hash, and
//! gets a new try at once.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use jewelcase_ffmpeg::Config;
use rusqlite::Connection;
use tempfile::NamedTempFile;
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::db::Database;
use crate::db::libraries::LibraryFile;

/// A resize that takes longer than this is killed: the image is broken or the host is stuck.
/// Since it may be the host, a timeout is not remembered the way a failure is.
const RENDER_TIMEOUT: Duration = Duration::from_secs(30);

/// Where resized images go and how to make them.
pub struct Images {
    cache: PathBuf,
    ffmpeg: Config,
    /// Bounds the resizes running at once. Artwork is an interactive request, so it runs outside
    /// the pool of background ffmpeg children, at normal priority and under its own bound
    /// (`design/scanning.md` §10).
    renders: Semaphore,
}

impl Images {
    /// Resizes into `cache` with the ffmpeg that `ffmpeg` names, as many images at once as the
    /// host has cores.
    pub fn new(cache: PathBuf, ffmpeg: Config) -> Images {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        Images { cache, ffmpeg, renders: Semaphore::new(cores) }
    }

    /// The resized copy of `library`'s image with content `hash` at `size`, from the file
    /// `source`, resizing it first if it is not cached yet. `None` when `source` is missing or
    /// reached through a symlink. A source that is not a regular file, or may not be read, is an
    /// error, and so is one ffmpeg cannot decode.
    pub async fn resized(
        &self,
        library: i64,
        hash: &[u8],
        size: u32,
        source: LibraryFile,
    ) -> anyhow::Result<Option<PathBuf>> {
        anyhow::ensure!(!hash.is_empty(), "the image has no content hash");
        let hex = hex(hash);
        // Split by the first byte, so no one directory holds every image.
        let dir = self.cache.join(library.to_string()).join(&hex[..2]);
        let path = dir.join(format!("{hex}-{size}.jpg"));
        let failed = dir.join(format!("{hex}-{size}.failed"));
        if made(&path, &failed).await? {
            return Ok(Some(path));
        }
        let _permit = self.renders.acquire().await?;
        // Another request may have made it, or failed to, while this one waited.
        if made(&path, &failed).await? {
            return Ok(Some(path));
        }
        // Checked at render time, since a file can become a symlink after the scan saw it. None
        // of the ways a file fails to open is the image's fault, so none is remembered.
        let Some(real) = open(source.clone()).await? else {
            return Ok(None);
        };
        tokio::fs::create_dir_all(&dir).await?;
        let output = self.render(size, &real).await?;
        if output.status.success() && !output.stdout.is_empty() {
            return keep(dir, path, output.stdout).await.map(Some);
        }
        // ffmpeg fails the same way on a file that went away as it read, as when a share drops,
        // as on a broken one; only a file that still opens was refused for its content.
        if open(source).await?.is_none() {
            return Ok(None);
        }
        // Only an ffmpeg that ran to the end is remembered. One killed by a signal, such as the
        // kernel reclaiming memory, may succeed next time.
        if output.status.code().is_some() {
            tokio::fs::write(&failed, []).await?;
        }
        anyhow::bail!(
            "ffmpeg failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }

    /// Resizes the image at `source` to fit `size`, as JPEG. An embedded image's source is its
    /// track file, and ffmpeg reads the attached picture from that the same way.
    async fn render(&self, size: u32, source: &Path) -> anyhow::Result<Output> {
        let child = Command::from(self.ffmpeg.resize_command(source, size))
            // A timed-out render drops the child, and dropping it kills the process.
            .kill_on_drop(true)
            .spawn()?;
        Ok(tokio::time::timeout(RENDER_TIMEOUT, child.wait_with_output()).await??)
    }

    /// Deletes from the cache everything but the copies of images `db` holds: a library's copies
    /// of images it no longer holds, the folders of libraries that are gone, the files of renders
    /// cut short, and every failure marker, so a failure that may pass, such as a file read as a
    /// tagger rewrote it, is tried again after a restart. Call it before the server takes
    /// requests, since it would delete the file of a render still being written.
    pub async fn sweep(&self, db: &Database) -> anyhow::Result<()> {
        let held = db.read(held).await?;
        let cache = self.cache.clone();
        Ok(tokio::task::spawn_blocking(move || sweep(&cache, &held)).await??)
    }
}

/// Whether the resized copy at `path` is there already; an error if `failed` marks it as one
/// ffmpeg cannot make.
async fn made(path: &Path, failed: &Path) -> anyhow::Result<bool> {
    if tokio::fs::try_exists(path).await? {
        return Ok(true);
    }
    anyhow::ensure!(
        !tokio::fs::try_exists(failed).await?,
        "ffmpeg could not read this image before"
    );
    Ok(false)
}

/// The real path of `source` once it opens, or `None` when it is missing or reached through a
/// symlink.
async fn open(source: LibraryFile) -> anyhow::Result<Option<PathBuf>> {
    match tokio::task::spawn_blocking(move || source.open()).await? {
        Ok(real) => Ok(Some(real)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Writes `jpeg` to `path` in `dir` whole, through a temporary file renamed into place, so no
/// request ever reads half a file. A crash before the rename leaves only the temporary file,
/// which [`Images::sweep`] deletes.
async fn keep(dir: PathBuf, path: PathBuf, jpeg: Vec<u8>) -> anyhow::Result<PathBuf> {
    let kept = tokio::task::spawn_blocking(move || -> io::Result<PathBuf> {
        let mut temp = NamedTempFile::new_in(dir)?;
        temp.write_all(&jpeg)?;
        temp.persist(&path)?;
        Ok(path)
    });
    Ok(kept.await??)
}

/// The content hash of every image, in hexadecimal, by library.
fn held(conn: &Connection) -> rusqlite::Result<HashMap<i64, HashSet<String>>> {
    let mut held: HashMap<i64, HashSet<String>> = HashMap::new();
    let mut select = conn.prepare("SELECT library_id, hash FROM images")?;
    let mut rows = select.query([])?;
    while let Some(row) = rows.next()? {
        held.entry(row.get(0)?).or_default().insert(hex(&row.get::<_, Vec<u8>>(1)?));
    }
    Ok(held)
}

/// Deletes everything in `cache` but the copies of the images `held` names, by library, failure
/// markers included.
fn sweep(cache: &Path, held: &HashMap<i64, HashSet<String>>) -> io::Result<()> {
    let libraries = match fs::read_dir(cache) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        libraries => libraries?,
    };
    for library in libraries {
        let library = library?.path();
        let id = library.file_name().and_then(|name| name.to_str()?.parse::<i64>().ok());
        match id.and_then(|id| held.get(&id)) {
            Some(hashes) => sweep_library(&library, hashes)?,
            None => remove(&library)?,
        }
    }
    Ok(())
}

/// Deletes everything in a library's folder, `library`, but the copies of the images with the
/// content `hashes`, failure markers included.
fn sweep_library(library: &Path, hashes: &HashSet<String>) -> io::Result<()> {
    for shard in fs::read_dir(library)? {
        let shard = shard?.path();
        if !shard.is_dir() {
            fs::remove_file(&shard)?;
            continue;
        }
        for file in fs::read_dir(&shard)? {
            let file = file?.path();
            // A copy is `{hash}-{size}.jpg`.
            let hash = file
                .file_name()
                .and_then(|name| Some(name.to_str()?.strip_suffix(".jpg")?.split_once('-')?.0));
            if !hash.is_some_and(|hash| hashes.contains(hash)) {
                remove(&file)?;
            }
        }
    }
    Ok(())
}

/// Deletes the file or folder at `path`, with everything in it.
fn remove(path: &Path) -> io::Result<()> {
    if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}

/// `bytes` in lowercase hexadecimal.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let digits = bytes.iter().flat_map(|byte| [byte >> 4, byte & 0xf]);
    digits.map(|digit| char::from(DIGITS[usize::from(digit)])).collect()
}
