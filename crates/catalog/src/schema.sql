-- The device's copy of a library's catalog: the server's catalog tables (crates/server/migrations)
-- with the same table and column names and types, minus what never leaves the server (paths,
-- roots, file stamps, scan and analysis state, plugin data, other users' data). Library content
-- does not depend on the user, so every library the account reaches shares these tables, each
-- row carrying its library_id. Counts the server keeps by trigger arrive as plain columns, and
-- foreign keys are left to the server, which has checked them.

-- The library's own counts, which the unfiltered lists' totals and the tag filters' plans read.
-- One row per library, kept by the library feed's `library` changes; no name, root or path.
CREATE TABLE libraries (
    id           INTEGER PRIMARY KEY,
    track_count  INTEGER NOT NULL DEFAULT 0,
    album_count  INTEGER NOT NULL DEFAULT 0,
    artist_count INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE images (
    id          INTEGER PRIMARY KEY,
    library_id  INTEGER NOT NULL,
    width       INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    -- A ThumbHash; NULL until the server has made one, and empty when the image could not be decoded
    placeholder BLOB
) STRICT;

CREATE TABLE artists (
    id                    INTEGER PRIMARY KEY,
    library_id            INTEGER NOT NULL,
    -- NULL is the unknown artist
    name                  TEXT,
    sort_key              BLOB    NOT NULL,
    image_id              INTEGER,
    biography             TEXT,
    album_count           INTEGER NOT NULL DEFAULT 0,
    track_count           INTEGER NOT NULL DEFAULT 0,
    available_track_count INTEGER NOT NULL DEFAULT 0,
    added_at              INTEGER NOT NULL
) STRICT;

CREATE INDEX artists_by_name ON artists (library_id, sort_key, id);
CREATE INDEX artists_by_added ON artists (library_id, added_at, id);

CREATE TABLE albums (
    id                    INTEGER PRIMARY KEY,
    library_id            INTEGER NOT NULL,
    -- NULL is the unknown album
    title                 TEXT,
    sort_key              BLOB    NOT NULL,
    artist_sort_key       BLOB    NOT NULL,
    type                  TEXT    NOT NULL DEFAULT 'album' CHECK (type IN ('album', 'ep', 'single', 'compilation')),
    labels                TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(labels) AND json_type(labels) = 'array'),
    image_id              INTEGER,
    release_date          TEXT,
    disc_total            INTEGER,
    track_total           INTEGER,
    disc_count            INTEGER NOT NULL DEFAULT 0,
    track_count           INTEGER NOT NULL DEFAULT 0,
    available_track_count INTEGER NOT NULL DEFAULT 0,
    duration_us           INTEGER NOT NULL DEFAULT 0,
    loudness_lufs         REAL,
    peak_dbtp             REAL,
    added_at              INTEGER NOT NULL
) STRICT;

CREATE INDEX albums_by_name ON albums (library_id, sort_key, id);
CREATE INDEX albums_by_artist ON albums (library_id, artist_sort_key, sort_key, id);
CREATE INDEX albums_by_added ON albums (library_id, added_at, id);
CREATE INDEX albums_by_release ON albums (library_id, release_date, sort_key, id);
CREATE INDEX albums_by_duration ON albums (library_id, duration_us, id);

CREATE TABLE album_artists (
    library_id INTEGER NOT NULL,
    album_id   INTEGER NOT NULL,
    position   INTEGER NOT NULL,
    artist_id  INTEGER NOT NULL,
    PRIMARY KEY (album_id, position)
) STRICT, WITHOUT ROWID;

CREATE INDEX album_artists_by_artist ON album_artists (artist_id, album_id);

CREATE TABLE album_credits (
    library_id  INTEGER NOT NULL,
    album_id    INTEGER NOT NULL,
    artist_id   INTEGER NOT NULL,
    track_count INTEGER NOT NULL,
    PRIMARY KEY (album_id, artist_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX album_credits_by_artist ON album_credits (artist_id, album_id);

CREATE TABLE album_discs (
    library_id  INTEGER NOT NULL,
    album_id    INTEGER NOT NULL,
    disc_number INTEGER NOT NULL CHECK (disc_number >= 0),
    track_count INTEGER NOT NULL,
    track_total INTEGER,
    PRIMARY KEY (album_id, disc_number)
) STRICT, WITHOUT ROWID;

CREATE TABLE tracks (
    id              INTEGER PRIMARY KEY,
    library_id      INTEGER NOT NULL,
    album_id        INTEGER NOT NULL,
    file_size       INTEGER NOT NULL,
    missing_since   INTEGER,
    title           TEXT    NOT NULL,
    sort_key        BLOB    NOT NULL,
    artist_sort_key BLOB    NOT NULL,
    album_sort_key  BLOB    NOT NULL,
    disc_number     INTEGER NOT NULL DEFAULT 0 CHECK (disc_number >= 0),
    track_number    INTEGER,
    track_total     INTEGER,
    disc_total      INTEGER,
    release_date    TEXT,
    explicit        INTEGER NOT NULL DEFAULT 0 CHECK (explicit IN (0, 1)),
    lyrics_kind     TEXT    NOT NULL DEFAULT 'none' CHECK (lyrics_kind IN ('none', 'plain', 'synced')),
    codec           TEXT    NOT NULL,
    container       TEXT    NOT NULL,
    lossless        INTEGER NOT NULL CHECK (lossless IN (0, 1)),
    bitrate_kbps    INTEGER,
    sample_rate_hz  INTEGER NOT NULL,
    bit_depth       INTEGER,
    channels        INTEGER NOT NULL,
    duration_us     INTEGER NOT NULL,
    loudness_lufs   REAL,
    peak_dbtp       REAL,
    image_id        INTEGER,
    added_at        INTEGER NOT NULL
) STRICT;

CREATE INDEX tracks_by_name ON tracks (library_id, sort_key, id);
CREATE INDEX tracks_by_artist ON tracks (library_id, artist_sort_key, sort_key, id);
CREATE INDEX tracks_by_album_order ON tracks (album_id, disc_number, ifnull(track_number, 9223372036854775807), id);
CREATE INDEX tracks_by_album ON tracks (library_id, album_sort_key, album_id, disc_number, ifnull(track_number, 9223372036854775807), id);
CREATE INDEX tracks_by_added ON tracks (library_id, added_at, id);
CREATE INDEX tracks_by_release ON tracks (library_id, release_date, album_sort_key, album_id, disc_number, ifnull(track_number, 9223372036854775807), id);
CREATE INDEX tracks_by_duration ON tracks (library_id, duration_us, id);

CREATE TABLE track_artists (
    library_id  INTEGER NOT NULL,
    track_id    INTEGER NOT NULL,
    position    INTEGER NOT NULL,
    artist_id   INTEGER NOT NULL,
    -- The artist's name as written in this file
    artist_name TEXT,
    PRIMARY KEY (track_id, position)
) STRICT, WITHOUT ROWID;

CREATE INDEX track_artists_by_artist ON track_artists (artist_id, track_id);

-- Server-wide, like the server's: every type files carry, the name a plugin's type is shown by
CREATE TABLE tag_types (
    key  TEXT PRIMARY KEY,
    name TEXT
) STRICT;

CREATE TABLE tags (
    id           INTEGER PRIMARY KEY,
    library_id   INTEGER NOT NULL,
    type         TEXT    NOT NULL,
    name         TEXT    NOT NULL,
    sort_key     BLOB    NOT NULL,
    track_count  INTEGER NOT NULL DEFAULT 0,
    album_count  INTEGER NOT NULL DEFAULT 0,
    artist_count INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX tags_by_name ON tags (library_id, type, sort_key, id);
CREATE INDEX tags_by_track_count ON tags (library_id, type, track_count, id);

CREATE TABLE track_tags (
    library_id INTEGER NOT NULL,
    track_id   INTEGER NOT NULL,
    tag_id     INTEGER NOT NULL,
    -- The tag as written in this file
    tag_name   TEXT    NOT NULL,
    PRIMARY KEY (track_id, tag_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX track_tags_by_tag ON track_tags (tag_id, track_id);

CREATE TABLE album_tags (
    library_id  INTEGER NOT NULL,
    album_id    INTEGER NOT NULL,
    tag_id      INTEGER NOT NULL,
    track_count INTEGER NOT NULL,
    PRIMARY KEY (album_id, tag_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX album_tags_by_tag ON album_tags (tag_id, album_id);

CREATE TABLE artist_tags (
    library_id  INTEGER NOT NULL,
    artist_id   INTEGER NOT NULL,
    tag_id      INTEGER NOT NULL,
    track_count INTEGER NOT NULL,
    PRIMARY KEY (artist_id, tag_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX artist_tags_by_tag ON artist_tags (tag_id, artist_id);

-- The account's own playlists, which the account feed (`GET /me/changes`) carries: only the
-- signed-in user's, so `user_id` is always that user's own. Never in a library's snapshot, which
-- every account that reaches the library shares.
CREATE TABLE playlists (
    id          INTEGER PRIMARY KEY,
    library_id  INTEGER NOT NULL,
    user_id     INTEGER NOT NULL,
    title       TEXT    NOT NULL,
    -- The title as search folds it, which a name filter looks inside
    folded      TEXT    NOT NULL,
    sort_key    BLOB    NOT NULL,
    description TEXT,
    pinned      INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
    version     INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    track_count INTEGER NOT NULL DEFAULT 0,
    duration_us INTEGER NOT NULL DEFAULT 0,
    -- The artwork's key as the API gives it: an image ID, or a mosaic of four
    image_key   TEXT,
    -- 1 for a pinned playlist and NULL for the rest, so a list puts the pinned first in both orders
    pin_rank    INTEGER GENERATED ALWAYS AS (nullif(pinned, 0)) VIRTUAL
) STRICT;

CREATE INDEX playlists_by_name ON playlists (user_id, library_id, pin_rank, sort_key, id);
CREATE INDEX playlists_by_created ON playlists (user_id, library_id, pin_rank, created_at, id);
CREATE INDEX playlists_by_modified ON playlists (user_id, library_id, pin_rank, modified_at, id);

-- A playlist's tracks in the user's order: `key` orders an item among its playlist's items
CREATE TABLE playlist_items (
    playlist_id INTEGER NOT NULL,
    key         INTEGER NOT NULL,
    library_id  INTEGER NOT NULL,
    track_id    INTEGER NOT NULL,
    added_at    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, key)
) STRICT, WITHOUT ROWID;
