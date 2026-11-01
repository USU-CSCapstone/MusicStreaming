//! Resized artwork, made by ffmpeg once per size and kept in `cache/images`, in a folder for
//! each library and named by the image's content hash. Two libraries holding the same artwork
//! each keep their own copy, so how fast one answers says nothing of what the other has viewed
//! (`requirements/general.md` §3.6). A playlist's mosaic of four images is kept in the library's
//! folder `m`, named by their IDs, and the sweep keeps it while the library holds all four. An
//! image's ID changes whenever its content does, so a cached copy never goes stale. An image
//! ffmpeg cannot decode leaves a `.failed` marker instead, so it is not tried again on every
//! request until the next start, when [`Images::sweep`] deletes every marker along with what no
//! image needs any more; changed content has a new hash, and gets a new try at once.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use jewelcase_ffmpeg::Config;
use jewelcase_scanner::FileToDecode;
use rusqlite::Connection;
use tempfile::NamedTempFile;
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::db::Database;

/// A resize that takes longer than this is killed: the image is broken or the host is stuck.
/// Since it may be the host, a timeout is not remembered the way a failure is.
const RENDER_TIMEOUT: Duration = Duration::from_secs(30);

/// The folder of a library's mosaics, which no two-digit shard of a hash is named like.
const MOSAICS: &str = "m";

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
        source: FileToDecode,
    ) -> anyhow::Result<Option<PathBuf>> {
        anyhow::ensure!(!hash.is_empty(), "the image has no content hash");
        let hex = hex(hash);
        // Split by the first byte, so no one directory holds every image.
        let dir = self.cache.join(library.to_string()).join(&hex[..2]);
        let stem = format!("{hex}-{size}");
        self.cached(dir, &stem, vec![source], |real| self.ffmpeg.resize_command(&real[0], size))
            .await
    }

    /// The mosaic of `library`'s images `ids`, a 2×2 grid read from their files `sources` in the
    /// same order, `size` pixels square, composed first if it is not cached yet. It is kept under
    /// the IDs, which change with the images' content, in the folder [`MOSAICS`]. `None`,
    /// errors and failures are as for [`Images::resized`], for any one of the sources.
    pub async fn mosaic(
        &self,
        library: i64,
        ids: [i64; 4],
        size: u32,
        sources: [FileToDecode; 4],
    ) -> anyhow::Result<Option<PathBuf>> {
        let dir = self.cache.join(library.to_string()).join(MOSAICS);
        let [a, b, c, d] = ids;
        let stem = format!("{a}-{b}-{c}-{d}-{size}");
        self.cached(dir, &stem, sources.into(), |real| self.ffmpeg.mosaic_command(real, size / 2))
            .await
    }

    /// The copy named `stem` in `dir`, made by the command `command` builds from the real paths
    /// of `sources` if it is not there yet. `None` when a source is missing or reached through a
    /// symlink; a failure ffmpeg ran to the end is remembered beside the copy.
    async fn cached(
        &self,
        dir: PathBuf,
        stem: &str,
        sources: Vec<FileToDecode>,
        command: impl FnOnce(&[PathBuf]) -> std::process::Command,
    ) -> anyhow::Result<Option<PathBuf>> {
        let path = dir.join(format!("{stem}.jpg"));
        let failed = dir.join(format!("{stem}.failed"));
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
        let Some(real) = open_all(&sources).await? else {
            return Ok(None);
        };
        tokio::fs::create_dir_all(&dir).await?;
        let output = render(command(&real)).await?;
        if output.status.success() && !output.stdout.is_empty() {
            return keep(dir, path, output.stdout).await.map(Some);
        }
        // ffmpeg fails the same way on a file that went away as it read, as when a share drops,
        // as on a broken one; only files that still open were refused for their content.
        if open_all(&sources).await?.is_none() {
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

    /// Deletes from the cache everything but the copies of images `db` holds and the mosaics made
    /// only of them: a library's copies of images it no longer holds, the folders of libraries that are gone, the files of renders
    /// cut short, and every failure marker, so a failure that may pass, such as a file read as a
    /// tagger rewrote it, is tried again after a restart. Call it before the server takes
    /// requests, since it would delete the file of a render still being written.
    pub async fn sweep(&self, db: &Database) -> anyhow::Result<()> {
        let held = db.read(held).await?;
        let cache = self.cache.clone();
        Ok(tokio::task::spawn_blocking(move || sweep(&cache, &held)).await??)
    }
}

/// Runs `command` to the end and returns its output, killing it after [`RENDER_TIMEOUT`].
async fn render(command: std::process::Command) -> anyhow::Result<Output> {
    let child = Command::from(command)
        // A timed-out render drops the child, and dropping it kills the process.
        .kill_on_drop(true)
        .spawn()?;
    Ok(tokio::time::timeout(RENDER_TIMEOUT, child.wait_with_output()).await??)
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

/// The real path of each of `sources` once they all open, or `None` when any is missing or
/// reached through a symlink.
async fn open_all(sources: &[FileToDecode]) -> anyhow::Result<Option<Vec<PathBuf>>> {
    let sources = sources.to_vec();
    let opened = tokio::task::spawn_blocking(move || {
        sources.iter().map(FileToDecode::open).collect::<io::Result<Vec<_>>>()
    });
    match opened.await? {
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

/// What a library holds of its images: their content hashes, in hexadecimal, and their IDs.
#[derive(Default)]
struct Held {
    hashes: HashSet<String>,
    ids: HashSet<i64>,
}

/// What every library holds of its images.
fn held(conn: &Connection) -> rusqlite::Result<HashMap<i64, Held>> {
    let mut held: HashMap<i64, Held> = HashMap::new();
    let mut select = conn.prepare("SELECT library_id, id, hash FROM images")?;
    let mut rows = select.query([])?;
    while let Some(row) = rows.next()? {
        let library = held.entry(row.get(0)?).or_default();
        library.ids.insert(row.get(1)?);
        library.hashes.insert(hex(&row.get::<_, Vec<u8>>(2)?));
    }
    Ok(held)
}

/// Deletes everything in `cache` but the copies of the images `held` names and the mosaics made
/// only of them, by library, failure markers included.
fn sweep(cache: &Path, held: &HashMap<i64, Held>) -> io::Result<()> {
    let libraries = match fs::read_dir(cache) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        libraries => libraries?,
    };
    for library in libraries {
        let library = library?.path();
        let id = library.file_name().and_then(|name| name.to_str()?.parse::<i64>().ok());
        match id.and_then(|id| held.get(&id)) {
            Some(held) => sweep_library(&library, held)?,
            None => remove(&library)?,
        }
    }
    Ok(())
}

/// Deletes everything in a library's folder, `library`, but the copies of the images `held`
/// names and the mosaics made only of them, failure markers included.
fn sweep_library(library: &Path, held: &Held) -> io::Result<()> {
    for shard in fs::read_dir(library)? {
        let shard = shard?.path();
        if !shard.is_dir() {
            fs::remove_file(&shard)?;
            continue;
        }
        let mosaics = shard.file_name().is_some_and(|name| name == MOSAICS);
        for file in fs::read_dir(&shard)? {
            let file = file?.path();
            let stem = file.file_name().and_then(|name| name.to_str()?.strip_suffix(".jpg"));
            let keep = stem.is_some_and(|stem| {
                if mosaics {
                    // A mosaic is `{id}-{id}-{id}-{id}-{size}.jpg`.
                    let mut parts = stem.split('-');
                    let ids: Vec<_> = parts.by_ref().take(4).collect();
                    ids.len() == 4
                        && ids.iter().all(|id| id.parse().is_ok_and(|id| held.ids.contains(&id)))
                } else {
                    // A copy is `{hash}-{size}.jpg`.
                    stem.split_once('-').is_some_and(|(hash, _)| held.hashes.contains(hash))
                }
            });
            if !keep {
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
