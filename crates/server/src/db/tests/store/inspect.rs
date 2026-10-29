//! The SQLite store as the scanner's pipeline suite inspects it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use jewelcase_core::Format;
use jewelcase_scanner::testing::{InspectStore, InspectTrack};
use jewelcase_scanner::*;
use rusqlite::{OptionalExtension, Result};

use super::libraries;
use crate::db::Text;
use crate::db::store::SqliteStore;

/// The format of a stored codec and container.
fn format_of(codec: &str, container: &str) -> Format {
    match (codec, container) {
        ("flac", _) => Format::Flac,
        ("alac", _) => Format::Alac,
        ("mp3", _) => Format::Mp3,
        ("aac", _) => Format::Aac,
        ("vorbis", _) => Format::Vorbis,
        ("opus", _) => Format::Opus,
        (_, "aiff") => Format::Aiff,
        _ => Format::Wav,
    }
}

impl InspectStore for SqliteStore {
    fn inspect_tracks(&self, lib: LibraryId) -> Vec<InspectTrack> {
        self.db().read_blocking(move |conn| {
        let roots: std::collections::HashMap<i64, PathBuf> = libraries::roots(conn, lib)
            .unwrap()
            .into_iter()
            .map(|r| (r.id, r.path))
            .collect();
        let mut stmt = conn
            .prepare(
                "SELECT t.id, t.root_id, t.path, t.missing_since IS NOT NULL, t.title, t.album_title, t.sort_key, \
                 t.file_artist_sort_key, t.codec, t.container, t.channels, t.sample_rate_hz, t.analyzer_version > 0, \
                 t.loudness_lufs IS NOT NULL, \
                 (SELECT image_id FROM albums a WHERE a.id = t.album_id) IS NOT NULL, \
                 EXISTS (SELECT 1 FROM track_artists ta JOIN artists ar ON ar.id = ta.artist_id \
                         WHERE ta.track_id = t.id AND ar.biography IS NOT NULL), \
                 t.lyrics_kind <> 'none' \
                 FROM tracks t WHERE t.library_id = ?1 ORDER BY t.path",
            )
            .unwrap();
        let rows = stmt
            .query_map([lib], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Vec<u8>>(6)?,
                    r.get::<_, Vec<u8>>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, i64>(10)?,
                    r.get::<_, i64>(11)?,
                    r.get::<_, bool>(12)?,
                    r.get::<_, bool>(13)?,
                    r.get::<_, bool>(14)?,
                    r.get::<_, bool>(15)?,
                    r.get::<_, bool>(16)?,
                ))
            })
            .unwrap();
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                root_id,
                rel,
                missing,
                title,
                album,
                sort_key,
                artist_sort_key,
                codec,
                container,
                channels,
                rate,
                analyzed,
                measured,
                art,
                bio,
                lyr,
            ) = row.unwrap();
            let artists: Vec<String> = {
                let mut s = conn
                    .prepare(
                        "SELECT ar.name FROM track_artists ta JOIN artists ar ON ar.id = ta.artist_id \
                         WHERE ta.track_id = ?1 AND ar.name IS NOT NULL ORDER BY ta.position",
                    )
                    .unwrap();
                s.query_map([id], |r| r.get(0))
                    .unwrap()
                    .collect::<std::result::Result<_, _>>()
                    .unwrap()
            };
            let album_artists: Vec<String> = {
                let mut s = conn
                    .prepare(
                        "SELECT ar.name FROM tracks t JOIN album_artists aa ON aa.album_id = t.album_id \
                         JOIN artists ar ON ar.id = aa.artist_id \
                         WHERE t.id = ?1 AND ar.name IS NOT NULL ORDER BY aa.position",
                    )
                    .unwrap();
                s.query_map([id], |r| r.get(0))
                    .unwrap()
                    .collect::<std::result::Result<_, _>>()
                    .unwrap()
            };
            let tags: BTreeMap<String, Vec<String>> = {
                let mut s = conn
                    .prepare(
                        "SELECT tag.type, tagged.tag_name FROM track_tags tagged \
                         JOIN tags tag ON tag.id = tagged.tag_id WHERE tagged.track_id = ?1 \
                         ORDER BY tag.type, tagged.tag_name",
                    )
                    .unwrap();
                let mut tags = BTreeMap::<String, Vec<String>>::new();
                for row in s.query_map([id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap() {
                    let (tag_type, name): (String, String) = row.unwrap();
                    tags.entry(tag_type).or_default().push(name);
                }
                tags
            };
            out.push(InspectTrack {
                id,
                path: roots[&root_id].join(&rel),
                missing,
                title,
                artists,
                album,
                album_artists,
                has_artwork: art,
                has_biography: bio,
                has_lyrics: lyr,
                sort_title: String::from_utf8(sort_key).unwrap(),
                sort_artist: String::from_utf8(artist_sort_key).unwrap(),
                format: format_of(&codec, &container),
                channels: if channels == 0 {
                    None
                } else {
                    Some(u8::try_from(channels).unwrap())
                },
                sample_rate: if rate == 0 { None } else { Some(u32::try_from(rate).unwrap()) },
                analyzed,
                measured,
                tags,
            });
        }
        Ok(out)
        })
        .unwrap()
    }

    fn inspect_problems(&self, lib: LibraryId) -> Vec<Problem> {
        self.db().read_blocking(move |conn| {
        let roots: std::collections::HashMap<i64, PathBuf> = libraries::roots(conn, lib)
            .unwrap()
            .into_iter()
            .map(|r| (r.id, r.path))
            .collect();
        let mut stmt = conn
            .prepare(
                "SELECT p.root_id, p.path, g.kind, p.detail, p.seen_at FROM scan_problems p \
                 JOIN scan_problem_groups g ON g.id = p.group_id WHERE p.library_id = ?1 ORDER BY p.path",
            )
            .unwrap();
        stmt.query_map([lib], |r| {
            Ok(Problem {
                path: roots[&r.get::<_, i64>(0)?].join(r.get::<_, String>(1)?),
                kind: r.get::<_, Text<_>>(2)?.0,
                detail: r.get(3)?,
                seen_at: r.get(4)?,
            })
        })
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        })
        .unwrap()
    }

    fn inspect_placeholder(&self, lib: LibraryId, hash: &[u8]) -> Option<Vec<u8>> {
        let hash = hash.to_vec();
        self.db()
            .read_blocking(move |conn| {
                conn.query_row(
                    "SELECT placeholder FROM images WHERE library_id = ?1 AND hash = ?2",
                    rusqlite::params![lib, hash],
                    |r| r.get::<_, Option<Vec<u8>>>(0),
                )
                .optional()
            })
            .unwrap()
            .flatten()
    }

    fn age_tag_reads(&self, lib: LibraryId) {
        self.db()
            .write_blocking(move |tx| {
                tx.execute("UPDATE tracks SET tags_generation = 0 WHERE library_id = ?1", [lib])
            })
            .unwrap();
    }

    fn add_tag_type(&self, key: &str, field: &str) {
        let (key, field) = (key.to_owned(), field.to_owned());
        self.db()
            .write_blocking(move |tx| {
                tx.execute(
                    "INSERT INTO tag_types (key, name, plugin_id, field, since) \
                     SELECT ?1, ?1, 'test', ?2, ifnull(max(since), 0) + 1 FROM tag_types",
                    rusqlite::params![key, field],
                )
            })
            .unwrap();
    }

    fn inspect_scans(&self, lib: LibraryId) -> Vec<Scan> {
        self.scans(lib).unwrap()
    }

    fn feed_head(&self, lib: LibraryId) -> i64 {
        self.db()
            .read_blocking(move |conn| {
                conn.query_row(
                    "SELECT ifnull(max(seq), 0) FROM library_changes WHERE library_id = ?1",
                    [lib],
                    |r| r.get(0),
                )
            })
            .unwrap()
    }

    fn feed_tracks_after(&self, lib: LibraryId, head: i64) -> Vec<TrackId> {
        self.db()
            .read_blocking(move |conn| {
                conn.prepare(
                    "SELECT entity_id FROM library_changes \
                     WHERE library_id = ?1 AND entity_type = 'track' AND seq > ?2 ORDER BY seq",
                )?
                .query_map([lib, head], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>>>()
            })
            .unwrap()
            .into_iter()
            .map(|id| TrackId::try_from(id).unwrap())
            .collect()
    }
}
