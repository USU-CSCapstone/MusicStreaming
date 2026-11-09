//! What a user's hidden tracks take from the counts of what holds them, so that every count is
//! of what they see (`requirements/conventions.md` §8). The catalog stores each count once, for
//! everyone, so a user's is the stored one less what they hid of it. That is read from their
//! hidden tracks, which are few, so every query here costs what they hid, never the library.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, params};

use super::{HID, albums, artists};

/// What the hidden tracks take from each count, by what holds them. Nothing, for a user who hid
/// nothing.
#[derive(Default)]
pub struct Losses {
    pub library: LibraryLoss,
    pub albums: HashMap<i64, AlbumLoss>,
    pub artists: HashMap<i64, ArtistLoss>,
    pub tags: HashMap<i64, TagLoss>,
}

/// From the library's counts.
#[derive(Default)]
pub struct LibraryLoss {
    pub tracks: i64,
    pub duration_us: i64,
    /// Albums and artists hidden whole ([`albums`], [`artists`]).
    pub albums: i64,
    pub artists: i64,
}

/// From an album's: its tracks, their duration, each disc's tracks and the discs left with none,
/// and the tags only its hidden tracks bring it.
#[derive(Default)]
pub struct AlbumLoss {
    pub tracks: i64,
    pub duration_us: i64,
    pub discs: HashMap<i64, i64>,
    pub discs_gone: i64,
    pub tags: HashSet<i64>,
}

/// From an artist's: the tracks crediting them, those of them on albums they do not own (their
/// appearances), their own albums hidden whole, and the tags only their hidden tracks bring them.
#[derive(Default)]
pub struct ArtistLoss {
    pub tracks: i64,
    pub featured: i64,
    pub albums: i64,
    pub tags: HashSet<i64>,
}

/// From a tag's: the tracks carrying it, and the albums and artists it no longer reaches.
#[derive(Default)]
pub struct TagLoss {
    pub tracks: i64,
    pub albums: i64,
    pub artists: i64,
}

/// What `user`'s hidden tracks in `library` take from every count. Each part is one query driven
/// by the hidden tracks, through the links of each to what holds it.
pub fn losses(conn: &Connection, user: i64, library: i64) -> rusqlite::Result<Losses> {
    let mut losses = Losses::default();
    let any: bool = conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM hidden_tracks WHERE user_id = ?1 AND library_id = ?2)",
        )?
        .query_row([user, library], |row| row.get(0))?;
    if !any {
        return Ok(losses);
    }
    let query = |sql: &str| conn.prepare_cached(&format!("{HID}{sql}"));

    // By album and disc: what each disc the album lists loses.
    let mut discs = query(
        "SELECT t.album_id, t.disc_number, count(*), sum(t.duration_us) \
         FROM hid CROSS JOIN tracks t ON t.id = hid.track_id GROUP BY t.album_id, t.disc_number",
    )?;
    let mut rows = discs.query(params![user, library])?;
    while let Some(row) = rows.next()? {
        let (album, disc, tracks, duration): (i64, i64, i64, i64) =
            (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
        let loss = losses.albums.entry(album).or_default();
        loss.tracks += tracks;
        loss.duration_us += duration;
        loss.discs.insert(disc, tracks);
        losses.library.tracks += tracks;
        losses.library.duration_us += duration;
    }
    // The discs left with no tracks, counted as `albums.disc_count` counts them: an unnumbered
    // disc is disc 1, so disc 1 is gone only once both are.
    let mut gone = query(
        "SELECT g.album_id FROM (SELECT t.album_id, max(t.disc_number, 1) AS disc, \
         count(*) AS hidden FROM hid CROSS JOIN tracks t ON t.id = hid.track_id \
         GROUP BY t.album_id, disc) g \
         WHERE g.hidden >= (SELECT sum(d.track_count) FROM album_discs d \
         WHERE d.album_id = g.album_id AND d.disc_number BETWEEN g.disc - (g.disc = 1) AND g.disc)",
    )?;
    let mut rows = gone.query(params![user, library])?;
    while let Some(row) = rows.next()? {
        losses.albums.entry(row.get(0)?).or_default().discs_gone += 1;
    }

    // The tags an album, or an artist, holds only through hidden tracks: as many of them carry it
    // there as `album_tags`, or `artist_tags`, counts.
    for (album, tag) in pairs(
        query(
            "SELECT t.album_id, tt.tag_id FROM hid CROSS JOIN tracks t ON t.id = hid.track_id \
             CROSS JOIN track_tags tt ON tt.track_id = t.id \
             CROSS JOIN album_tags at ON at.album_id = t.album_id AND at.tag_id = tt.tag_id \
             GROUP BY t.album_id, tt.tag_id HAVING count(*) >= max(at.track_count)",
        )?,
        user,
        library,
    )? {
        losses.albums.entry(album).or_default().tags.insert(tag);
        losses.tags.entry(tag).or_default().albums += 1;
    }
    for (artist, tag) in pairs(
        query(
            "SELECT ta.artist_id, tt.tag_id FROM hid \
             CROSS JOIN track_artists ta ON ta.track_id = hid.track_id \
             CROSS JOIN track_tags tt ON tt.track_id = hid.track_id \
             CROSS JOIN artist_tags art ON art.artist_id = ta.artist_id AND art.tag_id = tt.tag_id \
             GROUP BY ta.artist_id, tt.tag_id HAVING count(*) >= max(art.track_count)",
        )?,
        user,
        library,
    )? {
        losses.artists.entry(artist).or_default().tags.insert(tag);
        losses.tags.entry(tag).or_default().artists += 1;
    }

    // The tracks crediting each artist, and those of them on albums the artist does not own.
    let mut credits = query(
        "SELECT ta.artist_id, count(*), sum(NOT EXISTS (SELECT 1 FROM album_artists aa \
           WHERE aa.album_id = t.album_id AND aa.artist_id = ta.artist_id)) \
         FROM hid CROSS JOIN track_artists ta ON ta.track_id = hid.track_id \
         CROSS JOIN tracks t ON t.id = hid.track_id \
         WHERE ta.artist_id IS NOT NULL GROUP BY ta.artist_id",
    )?;
    let mut rows = credits.query(params![user, library])?;
    while let Some(row) = rows.next()? {
        let loss = losses.artists.entry(row.get(0)?).or_default();
        loss.tracks = row.get(1)?;
        loss.featured = row.get(2)?;
    }

    let mut carried = query(
        "SELECT tt.tag_id, count(*) FROM hid CROSS JOIN track_tags tt ON tt.track_id = hid.track_id \
         GROUP BY tt.tag_id",
    )?;
    let mut rows = carried.query(params![user, library])?;
    while let Some(row) = rows.next()? {
        losses.tags.entry(row.get(0)?).or_default().tracks = row.get(1)?;
    }

    // Albums hidden whole leave the library's count and their artists'; artists hidden whole
    // leave the library's.
    let whole = albums(conn, user, library)?;
    losses.library.albums = i64::try_from(whole.len()).unwrap_or(i64::MAX);
    losses.library.artists = i64::try_from(artists(conn, user, library)?.len()).unwrap_or(i64::MAX);
    let mut owners = conn.prepare_cached(
        "SELECT artist_id, count(*) FROM album_artists \
         WHERE album_id IN (SELECT value FROM json_each(?1)) GROUP BY artist_id",
    )?;
    let whole = serde_json::to_string(&whole).expect("IDs serialize");
    let mut rows = owners.query([whole])?;
    while let Some(row) = rows.next()? {
        losses.artists.entry(row.get(0)?).or_default().albums = row.get(1)?;
    }
    Ok(losses)
}

/// The hidden items of each of `user`'s playlists in `library`, by playlist: how many there are,
/// duplicates each counted, and their duration. Each hidden track is one probe of
/// `playlist_items_by_track`.
pub fn playlists(
    conn: &Connection,
    user: i64,
    library: i64,
) -> rusqlite::Result<HashMap<i64, (i64, i64)>> {
    conn.prepare_cached(&format!(
        "{HID}SELECT i.playlist_id, count(*), sum(t.duration_us) FROM hid \
         CROSS JOIN playlist_items i INDEXED BY playlist_items_by_track \
           ON i.library_id = ?2 AND i.track_id = hid.track_id \
         CROSS JOIN playlists p ON p.id = i.playlist_id AND p.user_id = ?1 \
         CROSS JOIN tracks t ON t.id = hid.track_id \
         GROUP BY i.playlist_id"
    ))?
    .query_map(params![user, library], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))?
    .collect()
}

/// The `(owner, tag)` pairs `select` finds for `user` in `library`.
fn pairs(
    mut select: rusqlite::CachedStatement<'_>,
    user: i64,
    library: i64,
) -> rusqlite::Result<Vec<(i64, i64)>> {
    select.query_map(params![user, library], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
}
