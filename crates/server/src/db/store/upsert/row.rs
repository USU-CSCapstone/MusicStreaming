//! A scanned file's track row: inserted, or updated in place, from one list of columns.

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use jewelcase_scanner::{Lyrics, TagSet, TrackRecord};
use rusqlite::types::Null;
use rusqlite::{OptionalExtension, Result, ToSql, Transaction};

use super::Credits;
use crate::db::store::roots::relative;
use crate::db::store::sidecars::sidecar_rows;
use crate::db::{Json, new_id};

/// The track columns a scan sets, both when it inserts the row and when it updates it. Each binds
/// by name, as `:column`. The insert also sets `id`, `library_id` and `added_at`, which an update
/// keeps.
const COLUMNS: [&str; 39] = [
    "album_id",
    "album_artist_ids",
    "album_title",
    "root_id",
    "path",
    "file_size",
    "file_mtime",
    "missing_since",
    "title",
    "sort_key",
    "file_artist_sort_key",
    "file_album_artist_sort_key",
    "file_album_sort_key",
    "disc_number",
    "track_number",
    "track_total",
    "disc_total",
    "release_date",
    "explicit",
    "compilation",
    "release_type",
    "labels",
    "isrc",
    "identifiers",
    "lyrics_kind",
    "codec",
    "container",
    "lossless",
    "bitrate_kbps",
    "sample_rate_hz",
    "bit_depth",
    "channels",
    "duration_us",
    "image_id",
    "image_embedded",
    "artist_image_id",
    "biography_id",
    "sidecar_stamp",
    "tags_generation",
];

/// Inserts track `:id` with [`COLUMNS`], or updates it with them when it is a track of
/// `:library_id`, returning its id and analyzer version. Either way the track copies the sort
/// keys of its album and of `:artist_id`, its first artist, and the copies then follow changes
/// to those keys (`design/database.md` §3). An update whose file size or mtime changed clears the analysis.
/// Unqualified column names on the right of the update's `=` are the row's values before it.
static UPSERT: LazyLock<String> = LazyLock::new(|| {
    let columns = COLUMNS.join(", ");
    let values = COLUMNS.map(|c| format!(":{c}")).join(", ");
    let updates = COLUMNS.map(|c| format!("{c} = excluded.{c}")).join(", ");
    format!(
        "INSERT INTO tracks (id, library_id, added_at, album_sort_key, artist_sort_key, {columns}) \
         VALUES (:id, :library_id, :added_at, (SELECT sort_key FROM albums WHERE id = :album_id), \
                 (SELECT sort_key FROM artists WHERE id = :artist_id), {values}) \
         ON CONFLICT (id) DO UPDATE SET \
         album_sort_key = excluded.album_sort_key, artist_sort_key = excluded.artist_sort_key, \
         analyzer_version = CASE WHEN file_size IS excluded.file_size AND file_mtime IS excluded.file_mtime \
         THEN analyzer_version ELSE 0 END, \
         loudness_lufs = CASE WHEN file_size IS excluded.file_size AND file_mtime IS excluded.file_mtime \
         THEN loudness_lufs END, \
         peak_dbtp = CASE WHEN file_size IS excluded.file_size AND file_mtime IS excluded.file_mtime \
         THEN peak_dbtp END, \
         {updates} \
         WHERE tracks.library_id = excluded.library_id \
         RETURNING id, analyzer_version"
    )
});

/// Upserts `rec` as a track of `lib` under root `root_id` ([`UPSERT`]), on the album `credits`
/// place it on and listing their album artists in order, with the art and sidecars it resolved
/// and, when new, added at `now`. Returns its id, drawn when `rec` has none, or `None` when `rec`
/// names another library's track. A track that existed and is back at analyzer version 0 loses
/// its waveform.
pub(super) fn write_row(
    tx: &Transaction<'_>,
    lib: i64,
    root_id: i64,
    rec: &TrackRecord,
    credits: &Credits<'_>,
    now: i64,
) -> Result<Option<i64>> {
    let sidecars = sidecar_rows(tx, lib, root_id, rec)?;
    let format = rec.properties.format;
    let lyrics_kind = match rec.lyrics() {
        None => "none",
        Some(Lyrics::Plain(_)) => "plain",
        Some(Lyrics::Synced(_)) => "synced",
    };
    let id = match rec.id {
        Some(id) => id,
        None => new_id()?,
    };
    let album_artist_ids: Vec<i64> = credits.album_artists.iter().map(|&(id, _)| id).collect();
    let first_artist = credits.artists.first().map(|&(artist_id, _)| artist_id);
    let values: [(&str, &dyn ToSql); COLUMNS.len() + 4] = [
        (":id", &id),
        (":library_id", &lib),
        (":added_at", &now),
        (":artist_id", &first_artist),
        (":album_id", &credits.album_id),
        (":album_artist_ids", &Json(album_artist_ids)),
        (":album_title", &rec.tags.album),
        (":root_id", &root_id),
        (":path", &relative(&rec.root, &rec.path)),
        (":file_size", &rec.size),
        (":file_mtime", &rec.mtime_ms),
        (":missing_since", &Null),
        (":title", &rec.display_title()),
        (":sort_key", &rec.sort.title.as_bytes()),
        (":file_artist_sort_key", &rec.sort.artist.as_bytes()),
        (":file_album_artist_sort_key", &rec.sort.album_artist.as_bytes()),
        (":file_album_sort_key", &rec.sort.album.as_bytes()),
        (":disc_number", &rec.tags.disc_number.unwrap_or(0)),
        (":track_number", &rec.tags.track_number),
        (":track_total", &rec.tags.track_total),
        (":disc_total", &rec.tags.disc_total),
        (":release_date", &rec.tags.release_date.map(|d| d.to_string())),
        (":explicit", &(rec.tags.explicit == Some(true))),
        (":compilation", &rec.tags.compilation),
        (":release_type", &rec.tags.release_type),
        (":labels", &Json(&rec.tags.labels)),
        (":isrc", &rec.tags.isrc),
        (":identifiers", &Json(identifiers(&rec.tags))),
        (":lyrics_kind", &lyrics_kind),
        (":codec", &format.codec_name()),
        (":container", &format.container_name()),
        (":lossless", &format.is_lossless()),
        (":bitrate_kbps", &rec.properties.bitrate_kbps),
        (":sample_rate_hz", &rec.properties.sample_rate.unwrap_or(0)),
        (":bit_depth", &rec.properties.bit_depth),
        (":channels", &rec.properties.channels.unwrap_or(0)),
        (":duration_us", &(rec.properties.duration_ms * 1000)),
        (":image_id", &sidecars.image_id),
        (":image_embedded", &sidecars.image_embedded),
        (":artist_image_id", &sidecars.artist_image_id),
        (":biography_id", &sidecars.biography_id),
        (":sidecar_stamp", &rec.sidecar_stamp),
        (":tags_generation", &rec.tags_generation),
    ];
    // rusqlite rejects a name the statement lacks and leaves an unbound one NULL, so with one
    // entry per placeholder, distinct names mean every placeholder is bound.
    debug_assert_eq!(
        values.iter().map(|(name, _)| name).collect::<HashSet<_>>().len(),
        values.len(),
        "each column binds once"
    );
    let Some((id, analyzer_version)) = tx
        .prepare_cached(UPSERT.as_str())?
        .query_row(values.as_slice(), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .optional()?
    else {
        return Ok(None);
    };
    // A track at version 0 has no analysis to keep, its waveform included; a new track has none.
    if rec.id.is_some() && analyzer_version == 0 {
        tx.prepare_cached("DELETE FROM track_waveforms WHERE track_id = ?1")?.execute([id])?;
    }
    Ok(Some(id))
}

/// The track's external identifiers, keyed as Picard names them.
fn identifiers(tags: &TagSet) -> BTreeMap<&'static str, &str> {
    [
        ("musicbrainz_recordingid", &tags.musicbrainz_recording_id),
        ("musicbrainz_albumid", &tags.musicbrainz_release_id),
    ]
    .into_iter()
    .filter_map(|(key, id)| Some((key, id.as_deref()?)))
    .collect()
}

#[cfg(test)]
mod tests {
    use jewelcase_scanner::TagSet;

    use super::identifiers;
    use crate::db::Json;
    use crate::db::testing::stored;

    #[test]
    fn identifiers_name_the_musicbrainz_ids_present() {
        let tags = TagSet {
            musicbrainz_recording_id: Some("r".into()),
            musicbrainz_release_id: Some("a".into()),
            ..TagSet::default()
        };
        assert_eq!(
            stored(Json(identifiers(&tags))),
            r#"{"musicbrainz_albumid":"a","musicbrainz_recordingid":"r"}"#
        );
        assert_eq!(stored(Json(identifiers(&TagSet::default()))), "{}");
    }
}
