//! The files the device-catalog scale gate loads (`web/e2e/catalog-scale.bench.ts`), made from the
//! generated library as a real server would serve them, on request.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use jewelcase_scanner::{Artwork, ArtworkSource, ImageInfo, SidecarImage, TrackRecord};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

use super::{build, quick_check};
use crate::api::changes;
use crate::db::store::SqliteStore;
use crate::db::testing::{library, scale};
use crate::db::{Database, new_id};

/// The share of the library's entities the catch-up pages hold, as a fraction's denominator.
const CATCH_UP: i64 = 4;

/// Writes a generated library ([`scale::size`] tracks, with art) as the server serves it, into
/// `target/catalog-scale`: `device.db.gz`, the catalog file of a library with a random ID built
/// exactly as `getLibraryCatalog` builds it ([`build`]); `device.db`, it inflated; `library`, the
/// library's ID; `sha256`, the ETag it is served with; and `changes/NNNN.json.gz`, the library
/// feed's first pages of 1000 from position 0, as `listLibraryChanges` answers them gzipped,
/// holding a quarter of its entities. Run it with
/// `cargo test --release -p jewelcase-server write_device_scale_files -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark data"]
fn write_device_scale_files() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/catalog-scale");
    let server = out.join("server.db");
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(out.join("changes")).unwrap();

    let id = new_id().unwrap();
    let db = Arc::new(Database::open(&server).unwrap());
    db.write_blocking(move |tx| library(tx, id, "Music", "/music")).unwrap();
    let store = SqliteStore::new(db.clone()).unwrap();
    let records = scale::records(Path::new("/music"), scale::size()).map(illustrated);
    let mut batches = scale::fill(&store, id, records);
    println!("scan batches: {}", scale::summary(&mut batches));
    // As the placeholder job leaves them: a ThumbHash of an album cover's usual length for every
    // image, empty for the one in a hundred that does not decode.
    db.write_blocking(|tx| {
        tx.execute_batch(
            "UPDATE images SET placeholder = \
             CASE WHEN id % 100 = 0 THEN x'' ELSE randomblob(25) END",
        )
    })
    .unwrap();
    let epoch = db.epoch();
    drop((store, db));

    let started = Instant::now();
    build(&server, &out.join("built"), id, epoch, quick_check).unwrap();
    println!("snapshot build: {:.1?}", started.elapsed());
    let built = fs::read_dir(out.join("built")).unwrap().next().unwrap().unwrap().path();
    let name = built.file_stem().unwrap().to_str().unwrap();
    fs::write(out.join("sha256"), name.rsplit('-').next().unwrap()).unwrap();
    let (zipped, device) = (out.join("device.db.gz"), out.join("device.db"));
    fs::rename(built, &zipped).unwrap();
    fs::remove_dir(out.join("built")).unwrap();
    io::copy(
        &mut GzDecoder::new(File::open(&zipped).unwrap()),
        &mut File::create(&device).unwrap(),
    )
    .unwrap();
    fs::write(out.join("library"), id.to_string()).unwrap();
    println!("device.db: {} bytes", fs::metadata(&device).unwrap().len());
    println!("device.db.gz: {} bytes", fs::metadata(&zipped).unwrap().len());

    let conn = Connection::open_with_flags(&server, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let entities: i64 = conn
        .query_row(
            "SELECT track_count + album_count + artist_count FROM libraries WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    let (mut after, mut written, mut bytes) = (0, 0, 0);
    for page in 0.. {
        let cursor = (after, (after > 0).then_some(epoch));
        let changes = changes::read(&conn, id, cursor, 1000, epoch).unwrap();
        let path = out.join(format!("changes/{page:04}.json.gz"));
        let mut gzip = GzEncoder::new(File::create(&path).unwrap(), Compression::default());
        serde_json::to_writer(&mut gzip, &changes).unwrap();
        gzip.finish().unwrap().flush().unwrap();
        bytes += fs::metadata(&path).unwrap().len();
        written += i64::try_from(changes.changes.len()).unwrap();
        after = changes.next;
        if written * CATCH_UP >= entities || after >= changes.head {
            break;
        }
    }
    println!("catch-up pages: {written} changes of {entities} entities, {bytes} bytes gzipped");
    drop(conn);
    for file in ["server.db", "server.db-wal", "server.db-shm"] {
        let _ = fs::remove_file(out.join(file));
    }
}

/// `rec` with the art a library mostly has: a `cover.jpg` in nine album folders in ten, which
/// every track of the album shows, and an `artist.jpg` in three artist folders in ten. Untagged
/// files have none.
fn illustrated(mut rec: TrackRecord) -> TrackRecord {
    if rec.tags.album.is_none() {
        return rec;
    }
    let album = rec.path.parent().unwrap().to_path_buf();
    let artist = album.parent().unwrap().to_path_buf();
    let cover = image(&album.join("cover.jpg"));
    if !cover.hash[0].is_multiple_of(10) {
        rec.artwork =
            Some(Artwork { source: ArtworkSource::Sidecar(album.join("cover.jpg")), info: cover });
    }
    let portrait = image(&artist.join("artist.jpg"));
    if portrait.hash[0] % 10 < 3 {
        rec.artist_image = Some(SidecarImage { path: artist.join("artist.jpg"), info: portrait });
    }
    rec
}

/// An image file at `path` as the scanner describes it: a hash of its own and a cover's usual
/// size.
fn image(path: &Path) -> ImageInfo {
    let hash = Sha256::digest(path.as_os_str().as_encoded_bytes()).to_vec();
    let side = [500, 600, 1000, 1200, 1400][usize::from(hash[1] % 5)];
    ImageInfo { hash, width: side, height: side }
}
