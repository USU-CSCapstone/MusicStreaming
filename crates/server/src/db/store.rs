//! The scanner's [`Store`] on SQLite (`design/scanning.md` §7,
//! `design/database.md` §3).
//!
//! Paths are stored relative to their root. Every batch is one transaction
//! that also maintains the derived data the schema assigns to the scanner
//! and appends the net effect to the change feed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use jewelcase_core::{fold, sort};
use jewelcase_scanner::problems::group_key;
use jewelcase_scanner::store::{Batch, IndexedFile, Store, StoreError};
use jewelcase_scanner::*;
use rusqlite::{Connection, OptionalExtension, Result, Transaction, params};

use super::catalog::{self, Touched};
use super::feed::{self, Entity, Op};
use super::libraries::{self, Root};
use super::{Database, DbError, now_ms};

pub struct SqliteStore {
    db: Arc<Database>,
    /// Active roots per library, refreshed on reconfiguration. Shared with the database jobs
    /// that need them.
    roots: Arc<Roots>,
}

impl SqliteStore {
    /// Loads the roots with a blocking read, so call it outside the async runtime.
    pub fn new(db: Arc<Database>) -> Result<SqliteStore, DbError> {
        let store = SqliteStore {
            db,
            roots: Arc::default(),
        };
        store.refresh_roots()?;
        Ok(store)
    }

    pub fn db(&self) -> &Arc<Database> {
        &self.db
    }

    /// Reload roots from `library_roots`. Call after roots change.
    pub fn refresh_roots(&self) -> Result<(), DbError> {
        let map = self
            .db
            .read_blocking(libraries::all)?
            .into_iter()
            .map(|lib| (lib.id, lib.roots))
            .collect();
        *self.roots.0.write().unwrap() = map;
        Ok(())
    }

    fn lib(id: &LibraryId) -> Option<i64> {
        id.parse().ok()
    }

    fn fail(what: &str, e: impl std::fmt::Display) {
        tracing::error!(error = %e, "store: {what} failed");
    }
}

/// Each library's active roots.
#[derive(Default)]
struct Roots(RwLock<HashMap<i64, Vec<Root>>>);

impl Roots {
    /// The root containing `path`, and the path relative to it.
    fn locate(&self, library: i64, path: &Path) -> Option<(i64, String)> {
        let roots = self.0.read().unwrap();
        let root = roots
            .get(&library)?
            .iter()
            .filter(|r| path.starts_with(&r.path))
            .max_by_key(|r| r.path.as_os_str().len())?;
        Some((root.id, relative(&root.path, path)))
    }

    fn root_by_path(&self, library: i64, root: &Path) -> Option<i64> {
        self.0
            .read()
            .unwrap()
            .get(&library)?
            .iter()
            .find(|r| r.path == root)
            .map(|r| r.id)
    }

    fn absolute(&self, library: i64, root_id: i64, rel: &str) -> Option<PathBuf> {
        let roots = self.0.read().unwrap();
        let root = roots.get(&library)?.iter().find(|r| r.id == root_id)?;
        Some(join(&root.path, rel))
    }
}

/// Relative path text with `/` separators; empty for the root itself.
fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn join(root: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    }
}

fn ms_to_time(ms: Option<i64>) -> Option<SystemTime> {
    ms.map(|m| SystemTime::UNIX_EPOCH + Duration::from_millis(m.max(0) as u64))
}

fn time_to_ms(t: Option<SystemTime>) -> Option<i64> {
    t.map(|t| system_time_ms(t) as i64)
}

fn trigger_str(t: Trigger) -> &'static str {
    match t {
        Trigger::Initial => "initial",
        Trigger::Watch => "watch",
        Trigger::Scheduled => "scheduled",
        Trigger::Manual => "manual",
        Trigger::Reconfigure => "reconfigure",
        Trigger::Restore => "restore",
    }
}

fn trigger_parse(s: &str) -> Trigger {
    match s {
        "initial" => Trigger::Initial,
        "watch" => Trigger::Watch,
        "scheduled" => Trigger::Scheduled,
        "reconfigure" => Trigger::Reconfigure,
        "restore" => Trigger::Restore,
        _ => Trigger::Manual,
    }
}

fn state_str(s: ScanState) -> &'static str {
    match s {
        ScanState::Queued => "queued",
        ScanState::Running => "running",
        ScanState::Completed => "completed",
        ScanState::Cancelled => "cancelled",
        ScanState::Suspended => "suspended",
    }
}

fn state_parse(s: &str) -> ScanState {
    match s {
        "queued" => ScanState::Queued,
        "running" => ScanState::Running,
        "cancelled" => ScanState::Cancelled,
        "suspended" => ScanState::Suspended,
        _ => ScanState::Completed,
    }
}

// ───────────────────────────── Store ─────────────────────────────

impl Store for SqliteStore {
    fn create_scan(&self, scan: &Scan) -> ScanId {
        let Some(lib) = Self::lib(&scan.library) else {
            return 0;
        };
        let scan = scan.clone();
        let scopes = serde_json::to_string(&scan.scopes).unwrap_or_else(|_| "[]".into());
        let result = self.db.write_blocking(move |tx| {
            tx.query_row(
                "INSERT INTO scans (id, library_id, trigger, state, scopes, cursor_scope, cursor_dir, \
                 files_seen, files_processed, added, updated, moved, missing, problems, current_path, \
                 created_at, started_at, finished_at) \
                 VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17) \
                 RETURNING id",
                params![
                    lib,
                    trigger_str(scan.trigger),
                    state_str(scan.state),
                    scopes,
                    scan.cursor.as_ref().map(|c| c.scope_index as i64),
                    scan.cursor
                        .as_ref()
                        .and_then(|c| c.after_directory.as_ref())
                        .map(|p| p.to_string_lossy().into_owned()),
                    scan.progress.files_seen as i64,
                    scan.progress.files_processed as i64,
                    scan.progress.added as i64,
                    scan.progress.updated as i64,
                    scan.progress.moved as i64,
                    scan.progress.missing as i64,
                    scan.progress.problems as i64,
                    scan.progress
                        .current_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    now_ms(),
                    time_to_ms(scan.started_at),
                    time_to_ms(scan.finished_at),
                ],
                |r| r.get::<_, i64>(0).map(|id| id as u64),
            )
        });
        result.unwrap_or_else(|e| {
            Self::fail("create_scan", e);
            0
        })
    }

    fn update_scan(&self, scan: &Scan) {
        let scan = scan.clone();
        let r = self.db.write_blocking(move |tx| {
            tx.execute(
            "UPDATE scans SET state = ?2, cursor_scope = ?3, cursor_dir = ?4, files_seen = ?5, files_processed = ?6, \
             added = ?7, updated = ?8, moved = ?9, missing = ?10, problems = ?11, current_path = ?12, \
             started_at = ?13, finished_at = ?14 WHERE id = ?1",
            params![
                scan.id as i64,
                state_str(scan.state),
                scan.cursor.as_ref().map(|c| c.scope_index as i64),
                scan.cursor
                    .as_ref()
                    .and_then(|c| c.after_directory.as_ref())
                    .map(|p| p.to_string_lossy().into_owned()),
                scan.progress.files_seen as i64,
                scan.progress.files_processed as i64,
                scan.progress.added as i64,
                scan.progress.updated as i64,
                scan.progress.moved as i64,
                scan.progress.missing as i64,
                scan.progress.problems as i64,
                scan.progress
                    .current_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
                time_to_ms(scan.started_at),
                time_to_ms(scan.finished_at),
            ],
        )
        });
        if let Err(e) = r {
            Self::fail("update_scan", e);
        }
    }

    fn interrupted_scans(&self, library: &LibraryId) -> Vec<Scan> {
        let Some(lib) = Self::lib(library) else {
            return Vec::new();
        };
        match self.db.read_blocking(move |conn| {
            read_scans(
                conn,
                "SELECT id, library_id, trigger, state, scopes, cursor_scope, cursor_dir, files_seen, files_processed, \
                 added, updated, moved, missing, problems, current_path, started_at, finished_at \
                 FROM scans WHERE library_id = ?1 AND state IN ('queued', 'running') ORDER BY created_at, id",
                lib,
            )
        }) {
            Ok(v) => v,
            Err(e) => {
                Self::fail("interrupted_scans", e);
                Vec::new()
            }
        }
    }

    fn lookup(&self, library: &LibraryId, path: &Path) -> Option<IndexedFile> {
        let lib = Self::lib(library)?;
        let (root_id, rel) = self.roots.locate(lib, path)?;
        self.db.read_blocking(move |conn| {
            conn.query_row(
            "SELECT id, file_size, file_mtime, missing_since FROM tracks WHERE root_id = ?1 AND path = ?2",
            params![root_id, rel],
            |r| {
                Ok(IndexedFile {
                    track_id: r.get::<_, i64>(0)? as u64,
                    size: r.get::<_, i64>(1)? as u64,
                    mtime_ms: r.get::<_, i64>(2)? as u64,
                    missing: r.get::<_, Option<i64>>(3)?.is_some(),
                })
            },
        )
        .optional()
        })
        .unwrap_or_else(|e| {
            Self::fail("lookup", e);
            None
        })
    }

    fn apply(&self, library: &LibraryId, batch: Batch) -> std::result::Result<(), StoreError> {
        let Some(lib) = Self::lib(library) else {
            return Err(StoreError(format!("unknown library id {library}")));
        };
        let roots = self.roots.clone();
        let result = self.db.write_blocking(move |tx| {
            let now = now_ms();
            let mut touched = Touched::default();
            for record in &batch.upserts {
                upsert_track(
                    &roots,
                    tx,
                    lib,
                    record,
                    batch.scan_id as i64,
                    now,
                    &mut touched,
                )?;
            }
            for id in batch.touched.iter().chain(&batch.returned) {
                tx.execute(
                    "UPDATE tracks SET last_seen_scan_id = ?2 WHERE id = ?1 AND library_id = ?3",
                    params![*id as i64, batch.scan_id as i64, lib],
                )?;
            }
            for id in &batch.returned {
                let id = *id as i64;
                let changed = tx.execute(
                    "UPDATE tracks SET missing_since = NULL, updated_at = ?2 \
                     WHERE id = ?1 AND library_id = ?3 AND missing_since IS NOT NULL",
                    params![id, now, lib],
                )?;
                if changed > 0 {
                    touched.track(tx, id)?;
                    feed::record(tx, lib, Entity::Track, id, Op::Upsert, now)?;
                }
            }
            for path in &batch.cleared {
                clear_problem(&roots, tx, lib, path)?;
            }
            for problem in &batch.problems {
                record_problem(&roots, tx, lib, problem)?;
            }
            catalog::recompute(tx, lib, &mut touched, now)
        });
        result.map_err(|e| {
            Self::fail("apply", &e);
            StoreError(e.to_string())
        })
    }

    fn mark_missing_unseen(&self, library: &LibraryId, scope: &Scope, scan_id: ScanId) -> usize {
        let Some(lib) = Self::lib(library) else {
            return 0;
        };
        let Some(root_id) = self.roots.root_by_path(lib, &scope.root) else {
            return 0;
        };
        let rel = relative(&scope.root, &scope.path);
        let depth = scope.depth;
        let result = self.db.write_blocking(move |tx| {
            let now = now_ms();
            // Scope clause without GLOB, so pattern characters in folder
            // names cannot widen or narrow it.
            let prefix = if rel.is_empty() {
                String::new()
            } else {
                format!("{rel}/")
            };
            let clause = match depth {
                Depth::Subtree => "substr(path, 1, ?3) = ?4",
                Depth::Directory => {
                    "substr(path, 1, ?3) = ?4 AND instr(substr(path, ?3 + 1), '/') = 0"
                }
            };
            let sql = format!(
                "SELECT id FROM tracks WHERE root_id = ?1 AND missing_since IS NULL \
                 AND (last_seen_scan_id IS NULL OR last_seen_scan_id <> ?2) AND {clause}"
            );
            let ids: Vec<i64> = {
                let mut stmt = tx.prepare(&sql)?;
                let rows = stmt.query_map(
                    params![root_id, scan_id as i64, prefix.len() as i64, prefix],
                    |r| r.get(0),
                )?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            let mut touched = Touched::default();
            for id in &ids {
                tx.execute(
                    "UPDATE tracks SET missing_since = ?2, updated_at = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
                touched.track(tx, *id)?;
                feed::record(tx, lib, Entity::Track, *id, Op::Upsert, now)?;
            }
            catalog::recompute(tx, lib, &mut touched, now)?;
            Ok(ids.len())
        });
        result.unwrap_or_else(|e| {
            Self::fail("mark_missing_unseen", e);
            0
        })
    }

    fn next_unanalyzed(
        &self,
        library: &LibraryId,
        analyzer_version: u32,
        limit: usize,
    ) -> Vec<(TrackId, PathBuf)> {
        let Some(lib) = Self::lib(library) else {
            return Vec::new();
        };
        let roots = self.roots.clone();
        let result = self.db.read_blocking(move |conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT id, root_id, path FROM tracks WHERE library_id = ?1 AND missing_since IS NULL \
                 AND (analyzer_version IS NULL OR analyzer_version < ?2) ORDER BY added_at DESC, id LIMIT ?3",
            )?;
            let rows =
                stmt.query_map(params![lib, analyzer_version as i64, limit as i64], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, root_id, rel) = row?;
                if let Some(path) = roots.absolute(lib, root_id, &rel) {
                    out.push((id as u64, path));
                }
            }
            Ok(out)
        });
        result.unwrap_or_else(|e| {
            Self::fail("next_unanalyzed", e);
            Vec::new()
        })
    }

    fn store_analysis(&self, library: &LibraryId, track_id: TrackId, result: AnalysisResult) {
        let Some(lib) = Self::lib(library) else {
            return;
        };
        let r = self.db.write_blocking(move |tx| {
            let now = now_ms();
            let id = track_id as i64;
            // No row means the track is not in this library, so nothing else may change.
            let Some(album_id) = tx
                .query_row(
                    "UPDATE tracks SET loudness_lufs = ?2, peak_dbtp = ?3, analyzed_at = ?4, analyzer_version = ?5 \
                     WHERE id = ?1 AND library_id = ?6 RETURNING album_id",
                    params![
                        id,
                        result.integrated_lufs,
                        result.true_peak_dbtp,
                        now,
                        result.analyzer_version as i64,
                        lib
                    ],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
            else {
                return Ok(());
            };
            if result.waveform.peaks.is_empty() {
                tx.execute("DELETE FROM track_waveforms WHERE track_id = ?1", [id])?;
            } else {
                tx.execute(
                    "INSERT OR REPLACE INTO track_waveforms (library_id, track_id, data) VALUES (?1, ?2, ?3)",
                    params![lib, id, result.waveform.to_blob()],
                )?;
            }
            catalog::recompute_album_loudness(tx, album_id)?;
            feed::record(tx, lib, Entity::Album, album_id, Op::Upsert, now)?;
            feed::record(tx, lib, Entity::Track, id, Op::Upsert, now)
        });
        if let Err(e) = r {
            Self::fail("store_analysis", e);
        }
    }

    fn next_without_placeholder(
        &self,
        library: &LibraryId,
        limit: usize,
    ) -> Vec<(Vec<u8>, PathBuf)> {
        let Some(lib) = Self::lib(library) else {
            return Vec::new();
        };
        let roots = self.roots.clone();
        let result = self.db.read_blocking(move |conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT hash, root_id, path FROM images \
                 WHERE library_id = ?1 AND placeholder IS NULL LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![lib, limit as i64], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (hash, root_id, rel) = row?;
                if let Some(path) = roots.absolute(lib, root_id, &rel) {
                    out.push((hash, path));
                }
            }
            Ok(out)
        });
        result.unwrap_or_else(|e| {
            Self::fail("next_without_placeholder", e);
            Vec::new()
        })
    }

    fn store_placeholder(&self, library: &LibraryId, hash: &[u8], placeholder: Vec<u8>) {
        let Some(lib) = Self::lib(library) else {
            return;
        };
        let hash = hash.to_vec();
        let r = self.db.write_blocking(move |tx| {
            let now = now_ms();
            let changed = !placeholder.is_empty();
            let Some(image) = tx
                .query_row(
                    "UPDATE images SET placeholder = ?3 WHERE library_id = ?1 AND hash = ?2 RETURNING id",
                    params![lib, hash, placeholder],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
            else {
                return Ok(());
            };
            // What clients see of an album or artist includes its image's placeholder. One
            // that could not be made leaves them as they were.
            if changed {
                for (entity, table) in [(Entity::Album, "albums"), (Entity::Artist, "artists")] {
                    let ids: Vec<i64> = tx
                        .prepare_cached(&format!(
                            "SELECT id FROM {table} WHERE library_id = ?1 AND image_id = ?2"
                        ))?
                        .query_map([lib, image], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    for id in ids {
                        feed::record(tx, lib, entity, id, Op::Upsert, now)?;
                    }
                }
            }
            Ok(())
        });
        if let Err(e) = r {
            Self::fail("store_placeholder", e);
        }
    }

    fn duplicate_candidates(&self, library: &LibraryId) -> Vec<DuplicateCandidate> {
        let Some(lib) = Self::lib(library) else {
            return Vec::new();
        };
        let roots = self.roots.clone();
        let result = self.db.read_blocking(move |conn| {
            let mut artists: HashMap<i64, Vec<(i64, String)>> = HashMap::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT track_id, position, artist_name FROM track_artists WHERE library_id = ?1 AND artist_name IS NOT NULL",
                )?;
                for row in stmt.query_map([lib], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })? {
                    let (t, pos, name) = row?;
                    artists.entry(t).or_default().push((pos, name));
                }
            }
            let mut album_artists: HashMap<i64, Vec<(i64, String)>> = HashMap::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT track_id, position, artist_name FROM track_album_artists WHERE library_id = ?1 AND artist_name IS NOT NULL",
                )?;
                for row in stmt.query_map([lib], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })? {
                    let (t, pos, name) = row?;
                    album_artists.entry(t).or_default().push((pos, name));
                }
            }
            let ordered = |m: &mut HashMap<i64, Vec<(i64, String)>>, id: i64| -> Vec<String> {
                let mut v = m.remove(&id).unwrap_or_default();
                v.sort();
                v.into_iter().map(|(_, n)| n).collect()
            };
            let mut stmt = conn.prepare(
                "SELECT id, root_id, path, title, album_title, track_number, disc_number FROM tracks \
                 WHERE library_id = ?1 AND missing_since IS NULL",
            )?;
            let rows = stmt.query_map([lib], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, root_id, rel, title, album, track_number, disc) = row?;
                let Some(path) = roots.absolute(lib, root_id, &rel) else {
                    continue;
                };
                out.push(DuplicateCandidate {
                    track_id: id as u64,
                    path,
                    title: Some(title),
                    artists: ordered(&mut artists, id),
                    album,
                    album_artists: ordered(&mut album_artists, id),
                    track_number: track_number.map(|n| n as u32),
                    disc_number: if disc == 0 { None } else { Some(disc as u32) },
                });
            }
            Ok(out)
        });
        result.unwrap_or_else(|e| {
            Self::fail("duplicate_candidates", e);
            Vec::new()
        })
    }
}

fn read_scans(conn: &Connection, sql: &str, lib: i64) -> Result<Vec<Scan>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([lib], |r| {
        let scopes: String = r.get(4)?;
        let cursor_scope: Option<i64> = r.get(5)?;
        let cursor_dir: Option<String> = r.get(6)?;
        Ok(Scan {
            id: r.get::<_, i64>(0)? as u64,
            library: r.get::<_, i64>(1)?.to_string(),
            trigger: trigger_parse(&r.get::<_, String>(2)?),
            state: state_parse(&r.get::<_, String>(3)?),
            scopes: serde_json::from_str(&scopes).unwrap_or_default(),
            cursor: cursor_scope.map(|s| Cursor {
                scope_index: s as usize,
                after_directory: cursor_dir.map(PathBuf::from),
            }),
            progress: ScanProgress {
                files_seen: r.get::<_, i64>(7)? as u64,
                files_processed: r.get::<_, i64>(8)? as u64,
                added: r.get::<_, i64>(9)? as u64,
                updated: r.get::<_, i64>(10)? as u64,
                moved: r.get::<_, i64>(11)? as u64,
                missing: r.get::<_, i64>(12)? as u64,
                problems: r.get::<_, i64>(13)? as u64,
                current_path: r.get::<_, Option<String>>(14)?.map(PathBuf::from),
            },
            started_at: ms_to_time(r.get(15)?),
            finished_at: ms_to_time(r.get(16)?),
        })
    })?;
    rows.collect()
}

impl SqliteStore {
    /// All scans for a library, newest first. For the admin API and tests.
    pub fn scans(&self, library: i64) -> Result<Vec<Scan>, DbError> {
        self.db.read_blocking(move |conn| {
            read_scans(
                conn,
                "SELECT id, library_id, trigger, state, scopes, cursor_scope, cursor_dir, files_seen, files_processed, \
                 added, updated, moved, missing, problems, current_path, started_at, finished_at \
                 FROM scans WHERE library_id = ?1 ORDER BY created_at DESC, id DESC",
                library,
            )
        })
    }
}

// ───────────────────────────── upsert ─────────────────────────────

fn upsert_track(
    roots: &Roots,
    tx: &Transaction<'_>,
    lib: i64,
    rec: &TrackRecord,
    scan_id: i64,
    now: i64,
    touched: &mut Touched,
) -> Result<()> {
    let Some(root_id) = roots.root_by_path(lib, &rec.root) else {
        tracing::warn!(path = %rec.path.display(), "track under an unknown root; skipped");
        return Ok(());
    };
    let rel = relative(&rec.root, &rec.path);
    if let Some(id) = rec.id {
        // Whatever it pointed at before may now be orphaned.
        touched.track(tx, id as i64)?;
    }

    // Artists: the unknown artist (key "") when a file names none.
    let credits: Vec<Option<&str>> = if rec.tags.artists.is_empty() {
        vec![None]
    } else {
        rec.tags.artists.iter().map(|s| Some(s.as_str())).collect()
    };
    let mut artist_ids = Vec::new();
    for name in &credits {
        artist_ids.push(find_or_create_artist(tx, lib, *name, now)?);
    }

    // Album artists: tagged, else the track artists, else unknown for a
    // compilation with no album-artist tag (`requirements/scanning.md` §2).
    let album_credits: Vec<Option<&str>> = if !rec.tags.album_artists.is_empty() {
        rec.tags
            .album_artists
            .iter()
            .map(|s| Some(s.as_str()))
            .collect()
    } else if rec.tags.compilation {
        vec![None]
    } else {
        credits.clone()
    };
    let mut album_artist_ids = Vec::new();
    for name in &album_credits {
        album_artist_ids.push(find_or_create_artist(tx, lib, *name, now)?);
    }
    let artists_key = fold::artists_key(album_credits.iter().map(|n| n.unwrap_or("")));
    let album_id = find_or_create_album(
        tx,
        lib,
        rec.tags.album.as_deref(),
        &artists_key,
        &rec.sort.album,
        &rec.sort.album_artist,
        now,
    )?;

    let identifiers = {
        let mut m = serde_json::Map::new();
        if let Some(v) = &rec.tags.musicbrainz_recording_id {
            m.insert("musicbrainz_recordingid".into(), v.clone().into());
        }
        if let Some(v) = &rec.tags.musicbrainz_release_id {
            m.insert("musicbrainz_albumid".into(), v.clone().into());
        }
        serde_json::Value::Object(m).to_string()
    };
    let lyrics = rec.lyrics();
    let lyrics_kind = match lyrics {
        None => "none",
        Some(l) if l.synced => "synced",
        Some(_) => "plain",
    };
    let format = rec.properties.format;

    // A new track (no id yet) gets a random one.
    let id: i64 = tx.query_row(
            "INSERT INTO tracks (id, library_id, album_id, album_title, fingerprint, root_id, path, file_size, file_mtime, \
             missing_since, last_seen_scan_id, title, sort_key, artist_sort_key, album_sort_key, disc_number, track_number, \
             track_total, disc_total, release_date, explicit, compilation, release_type, isrc, identifiers, lyrics_kind, \
             codec, container, lossless, bitrate_kbps, sample_rate_hz, bit_depth, channels, duration_us, added_at, updated_at) \
             VALUES (coalesce(?1, random() & 0x7FFFFFFFFFFFFFFF), ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8, NULL, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, \
             ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?33) \
             ON CONFLICT (id) DO UPDATE SET album_id = excluded.album_id, album_title = excluded.album_title, \
             root_id = excluded.root_id, path = excluded.path, file_size = excluded.file_size, file_mtime = excluded.file_mtime, \
             missing_since = NULL, last_seen_scan_id = excluded.last_seen_scan_id, title = excluded.title, \
             sort_key = excluded.sort_key, artist_sort_key = excluded.artist_sort_key, album_sort_key = excluded.album_sort_key, \
             disc_number = excluded.disc_number, track_number = excluded.track_number, track_total = excluded.track_total, \
             disc_total = excluded.disc_total, release_date = excluded.release_date, explicit = excluded.explicit, \
             compilation = excluded.compilation, release_type = excluded.release_type, isrc = excluded.isrc, \
             identifiers = excluded.identifiers, lyrics_kind = excluded.lyrics_kind, codec = excluded.codec, \
             container = excluded.container, lossless = excluded.lossless, bitrate_kbps = excluded.bitrate_kbps, \
             sample_rate_hz = excluded.sample_rate_hz, bit_depth = excluded.bit_depth, channels = excluded.channels, \
             duration_us = excluded.duration_us, updated_at = excluded.updated_at \
             RETURNING id",
            params![
                rec.id.map(|id| id as i64),
                lib,
                album_id,
                rec.tags.album,
                root_id,
                rel,
                rec.size as i64,
                rec.mtime_ms as i64,
                scan_id,
                rec.display_title(),
                rec.sort.title.as_bytes(),
                rec.sort.artist.as_bytes(),
                rec.sort.album.as_bytes(),
                rec.tags.disc_number.unwrap_or(0) as i64,
                rec.tags.track_number.map(|n| n as i64),
                rec.tags.track_total.map(|n| n as i64),
                rec.tags.disc_total.map(|n| n as i64),
                rec.tags.release_date.map(|d| d.to_string()),
                rec.tags.explicit == Some(true),
                rec.tags.compilation,
                rec.tags.release_type,
                rec.tags.isrc,
                identifiers,
                lyrics_kind,
                format.codec_name(),
                format.container_name(),
                format.is_lossless(),
                rec.properties.bitrate_kbps.map(|b| b as i64),
                rec.properties.sample_rate.unwrap_or(0) as i64,
                rec.properties.bit_depth.map(|b| b as i64),
                rec.properties.channels.unwrap_or(0) as i64,
                (rec.properties.duration_ms * 1000) as i64,
                now,
            ],
            |r| r.get(0),
        )?;

    // Links: replace wholesale.
    tx.execute("DELETE FROM track_artists WHERE track_id = ?1", [id])?;
    tx.execute("DELETE FROM track_album_artists WHERE track_id = ?1", [id])?;
    tx.execute("DELETE FROM track_tags WHERE track_id = ?1", [id])?;
    for (pos, (artist_id, name)) in artist_ids.iter().zip(&credits).enumerate() {
        tx.execute(
                "INSERT OR IGNORE INTO track_artists (library_id, track_id, position, artist_id, artist_name) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![lib, id, pos as i64, artist_id, name.map(str::trim)],
            )?;
    }
    for (pos, (artist_id, name)) in album_artist_ids.iter().zip(&album_credits).enumerate() {
        tx.execute(
                "INSERT OR IGNORE INTO track_album_artists (library_id, track_id, position, artist_id, artist_name) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![lib, id, pos as i64, artist_id, name.map(str::trim)],
            )?;
    }
    for genre in &rec.tags.genres {
        let tag_id = find_or_create_tag(tx, lib, genre)?;
        tx.execute(
                "INSERT OR IGNORE INTO track_tags (library_id, track_id, tag_id, tag_name) VALUES (?1, ?2, ?3, ?4)",
                params![lib, id, tag_id, genre.trim()],
            )?;
        touched.tags.insert(tag_id);
    }

    // Lyrics.
    tx.execute("DELETE FROM track_lyrics WHERE track_id = ?1", [id])?;
    if let Some(l) = lyrics {
        let synced = if l.synced {
            let lines = jewelcase_core::lrc::parse(&l.text);
            if lines.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&lines).unwrap())
            }
        } else {
            None
        };
        tx.execute(
                "INSERT INTO track_lyrics (library_id, track_id, plain, synced, embedded) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![lib, id, l.text, synced, rec.tags.lyrics.is_some()],
            )?;
    }

    // Artwork: an images row per distinct source; the album keeps the
    // first it is given (`requirements/scanning.md` §3.1).
    if let Some(art) = &rec.artwork {
        let (img_path, embedded) = match &art.source {
            ArtworkSource::Embedded => (rel.clone(), true),
            ArtworkSource::Sidecar(p) => (relative(&rec.root, p), false),
        };
        let image_id = upsert_image(tx, lib, root_id, &img_path, embedded, &art.info)?;
        tx.execute(
            "UPDATE albums SET image_id = ?2 WHERE id = ?1 AND image_id IS NULL",
            params![album_id, image_id],
        )?;
    }
    let artist_targets: &[i64] = if album_credits.iter().any(|c| c.is_some()) {
        &album_artist_ids
    } else {
        &artist_ids
    };
    if let Some(img) = &rec.artist_image {
        let image_id = upsert_image(
            tx,
            lib,
            root_id,
            &relative(&rec.root, &img.path),
            false,
            &img.info,
        )?;
        for artist_id in artist_targets {
            tx.execute(
                    "UPDATE artists SET image_id = ?2 WHERE id = ?1 AND image_id IS NULL AND name IS NOT NULL",
                    params![artist_id, image_id],
                )?;
        }
    }
    if let Some(bio) = &rec.artist_biography {
        for artist_id in artist_targets {
            tx.execute(
                    "UPDATE artists SET biography = ?2 WHERE id = ?1 AND biography IS NULL AND name IS NOT NULL",
                    params![artist_id, bio.text],
                )?;
        }
    }

    touched.albums.insert(album_id);
    touched
        .artists
        .extend(artist_ids.iter().chain(&album_artist_ids));
    feed::record(tx, lib, Entity::Track, id, Op::Upsert, now)?;
    Ok(())
}

fn record_problem(roots: &Roots, tx: &Transaction<'_>, lib: i64, problem: &Problem) -> Result<()> {
    let Some((root_id, rel)) = roots.locate(lib, &problem.path) else {
        return Ok(());
    };
    let root_path = roots
        .0
        .read()
        .unwrap()
        .get(&lib)
        .and_then(|rs| rs.iter().find(|r| r.id == root_id))
        .map(|r| r.path.clone())
        .unwrap_or_default();
    let key = group_key(problem.kind, &root_path, &problem.detail);
    let seen = system_time_ms(problem.seen_at) as i64;
    let group_id: i64 = tx.query_row(
        "INSERT INTO scan_problem_groups (id, library_id, kind, group_key, summary, count, first_seen_at, last_seen_at) \
         VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, 0, ?5, ?5) \
         ON CONFLICT (library_id, group_key) DO UPDATE SET last_seen_at = MAX(last_seen_at, excluded.last_seen_at) \
         RETURNING id",
        params![lib, problem.kind.as_str(), key, problem.detail, seen],
        |r| r.get(0),
    )?;
    let existing: Option<i64> = tx
        .query_row(
            "SELECT group_id FROM scan_problems WHERE root_id = ?1 AND path = ?2",
            params![root_id, rel],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(old) if old == group_id => {
            tx.execute(
                    "UPDATE scan_problems SET detail = ?3, seen_at = ?4 WHERE root_id = ?1 AND path = ?2",
                    params![root_id, rel, problem.detail, seen],
                )?;
        }
        _ => {
            // Insert into the new group before recounting the old one, so
            // a group is never emptied and deleted while still referenced.
            tx.execute(
                "DELETE FROM scan_problems WHERE root_id = ?1 AND path = ?2",
                params![root_id, rel],
            )?;
            tx.execute(
                    "INSERT INTO scan_problems (library_id, group_id, root_id, path, detail, seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![lib, group_id, root_id, rel, problem.detail, seen],
                )?;
            recount_group(tx, group_id)?;
            if let Some(old) = existing {
                recount_group(tx, old)?;
            }
        }
    }
    Ok(())
}

fn clear_problem(roots: &Roots, tx: &Transaction<'_>, lib: i64, path: &Path) -> Result<()> {
    let Some((root_id, rel)) = roots.locate(lib, path) else {
        return Ok(());
    };
    clear_problem_rel(tx, root_id, &rel)
}

fn clear_problem_rel(tx: &Transaction<'_>, root_id: i64, rel: &str) -> Result<()> {
    let group: Option<i64> = tx
        .query_row(
            "SELECT group_id FROM scan_problems WHERE root_id = ?1 AND path = ?2",
            params![root_id, rel],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(group_id) = group {
        tx.execute(
            "DELETE FROM scan_problems WHERE root_id = ?1 AND path = ?2",
            params![root_id, rel],
        )?;
        recount_group(tx, group_id)?;
    }
    Ok(())
}

fn recount_group(tx: &Transaction<'_>, group_id: i64) -> Result<()> {
    tx.execute(
        "UPDATE scan_problem_groups SET count = (SELECT COUNT(*) FROM scan_problems WHERE group_id = ?1) WHERE id = ?1",
        [group_id],
    )?;
    tx.execute(
        "DELETE FROM scan_problem_groups WHERE id = ?1 AND count = 0",
        [group_id],
    )?;
    Ok(())
}

// Each find-or-create reads first, since the key is almost always there already, and inserts
// only when it is not.

fn find_or_create_artist(
    tx: &Transaction<'_>,
    lib: i64,
    name: Option<&str>,
    now: i64,
) -> Result<i64> {
    let key = name.map(fold::name_key).unwrap_or_default();
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM artists WHERE library_id = ?1 AND name_key = ?2",
            params![lib, key],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    let sort_key = sort::sort_key(name.unwrap_or(""), None);
    tx.query_row(
        "INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at, updated_at) \
         VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, ?5, ?5) RETURNING id",
        params![lib, name.map(str::trim), key, sort_key.as_bytes(), now],
        |r| r.get(0),
    )
}

fn find_or_create_album(
    tx: &Transaction<'_>,
    lib: i64,
    title: Option<&str>,
    artists_key: &str,
    sort_key: &str,
    artist_sort_key: &str,
    now: i64,
) -> Result<i64> {
    let title_key = title.map(fold::name_key).unwrap_or_default();
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM albums WHERE library_id = ?1 AND title_key = ?2 AND artists_key = ?3",
            params![lib, title_key, artists_key],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    tx.query_row(
        "INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key, artist_sort_key, added_at, updated_at) \
         VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7) RETURNING id",
        params![lib, title.map(str::trim), title_key, artists_key, sort_key.as_bytes(), artist_sort_key.as_bytes(), now],
        |r| r.get(0),
    )
}

fn find_or_create_tag(tx: &Transaction<'_>, lib: i64, name: &str) -> Result<i64> {
    let key = fold::tag_key(name);
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM tags WHERE library_id = ?1 AND type = 'genre' AND name_key = ?2",
            params![lib, key],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    let sort_key = sort::sort_key(name, None);
    tx.query_row(
        "INSERT INTO tags (id, library_id, type, name, name_key, sort_key) \
         VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, 'genre', ?2, ?3, ?4) RETURNING id",
        params![lib, name.trim(), key, sort_key.as_bytes()],
        |r| r.get(0),
    )
}

fn upsert_image(
    tx: &Transaction<'_>,
    lib: i64,
    root_id: i64,
    rel: &str,
    embedded: bool,
    info: &ImageInfo,
) -> Result<i64> {
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM images WHERE library_id = ?1 AND hash = ?2",
            params![lib, info.hash],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    tx.query_row(
        "INSERT INTO images (id, library_id, hash, format, width, height, placeholder, root_id, path, embedded) \
         VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8) RETURNING id",
        params![lib, info.hash, info.format, info.width as i64, info.height as i64, root_id, rel, embedded],
        |r| r.get(0),
    )
}
