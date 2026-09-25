-- Library content: images, artists, albums, tracks, tags, lyrics, waveforms.
--
-- Every table carries library_id, and every link between two content rows is a composite
-- foreign key on (library_id, id), so a row can never point into another library
-- (requirements/general.md §3.6). Only the scanner writes these tables (requirements/general.md §3.2).

-- ───────────────────────────── Images ─────────────────────────────

-- Album covers and artist images, found per requirements/scanning.md §3.1–§3.2.
CREATE TABLE images (
    id          INTEGER PRIMARY KEY,
    library_id  INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    hash        BLOB    NOT NULL,
    format      TEXT    NOT NULL,
    width       INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    placeholder BLOB    NOT NULL,
    root_id     INTEGER NOT NULL,
    path        TEXT    NOT NULL,
    -- Whether the image is embedded in a track file
    embedded    INTEGER NOT NULL CHECK (embedded IN (0, 1)),
    UNIQUE (library_id, hash),
    UNIQUE (library_id, id),
    FOREIGN KEY (library_id, root_id) REFERENCES library_roots (library_id, id)
) STRICT;

-- ───────────────────────────── Artists ─────────────────────────────

CREATE TABLE artists (
    id               INTEGER PRIMARY KEY,
    library_id       INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    -- NULL is the unknown artist. Otherwise the most-used spelling of tracks.artist_name
    name             TEXT,
    -- Name split from the tag on semicolons, trimmed of outer whitespace, and Unicode case-folded
    name_key         TEXT    NOT NULL,
    sort_key         BLOB    NOT NULL,
    image_id         INTEGER,
    biography        TEXT,
    album_count      INTEGER NOT NULL DEFAULT 0,
    track_count      INTEGER NOT NULL DEFAULT 0,
    appearance_count INTEGER NOT NULL DEFAULT 0,
    available_track_count INTEGER NOT NULL DEFAULT 0,
    added_at         INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    UNIQUE (library_id, name_key),
    UNIQUE (library_id, id),
    FOREIGN KEY (library_id, image_id) REFERENCES images (library_id, id)
) STRICT;

CREATE INDEX artists_by_name ON artists (library_id, sort_key, id);
CREATE INDEX artists_by_added ON artists (library_id, added_at, id);

-- ───────────────────────────── Albums ─────────────────────────────

CREATE TABLE albums (
    id                    INTEGER PRIMARY KEY,
    library_id            INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    -- NULL is the unknown album. Otherwise the most-used spelling of tracks.album_title
    title                 TEXT,
    -- The title trimmed of outer whitespace and Unicode case-folded
    title_key             TEXT    NOT NULL,
    -- The album artists' name_keys, sorted so order does not matter, joined with ';'
    artists_key           TEXT    NOT NULL,
    sort_key              BLOB    NOT NULL,
    artist_sort_key       BLOB    NOT NULL,
    type                  TEXT    NOT NULL DEFAULT 'album' CHECK (type IN ('album', 'ep', 'single', 'compilation')),
    labels                TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(labels) AND json_type(labels) = 'array'),
    image_id              INTEGER,
    release_date          TEXT    CHECK (release_date GLOB '[0-9][0-9][0-9][0-9]' OR release_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]' OR release_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    disc_total            INTEGER,
    track_total           INTEGER,
    disc_count            INTEGER NOT NULL DEFAULT 0,
    track_count           INTEGER NOT NULL DEFAULT 0,
    available_track_count INTEGER NOT NULL DEFAULT 0,
    duration_us           INTEGER NOT NULL DEFAULT 0,
    loudness_lufs         REAL,
    peak_dbtp             REAL,
    added_at              INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL,
    UNIQUE (library_id, title_key, artists_key),
    UNIQUE (library_id, id),
    FOREIGN KEY (library_id, image_id) REFERENCES images (library_id, id)
) STRICT;

CREATE INDEX albums_by_name ON albums (library_id, sort_key, id);
CREATE INDEX albums_by_artist ON albums (library_id, artist_sort_key, sort_key, id);
CREATE INDEX albums_by_added ON albums (library_id, added_at, id);
CREATE INDEX albums_by_release ON albums (library_id, release_date, id);
CREATE INDEX albums_by_duration ON albums (library_id, duration_us, id);

CREATE TABLE album_artists (
    library_id INTEGER NOT NULL,
    album_id   INTEGER NOT NULL,
    position   INTEGER NOT NULL,
    artist_id  INTEGER NOT NULL,
    PRIMARY KEY (album_id, position),
    UNIQUE (album_id, artist_id),
    FOREIGN KEY (library_id, album_id) REFERENCES albums (library_id, id) ON DELETE CASCADE,
    FOREIGN KEY (library_id, artist_id) REFERENCES artists (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX album_artists_by_artist ON album_artists (artist_id, album_id);

CREATE TABLE album_discs (
    library_id  INTEGER NOT NULL,
    album_id    INTEGER NOT NULL,
    disc_number INTEGER NOT NULL CHECK (disc_number >= 0),
    track_count INTEGER NOT NULL,
    track_total INTEGER,
    PRIMARY KEY (album_id, disc_number),
    FOREIGN KEY (library_id, album_id) REFERENCES albums (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- ───────────────────────────── Tracks ─────────────────────────────

CREATE TABLE tracks (
    id              INTEGER PRIMARY KEY,
    library_id      INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    album_id        INTEGER NOT NULL,
    -- The album's title as written in this file
    album_title     TEXT,
    -- A unique fingerprint of the audio, for deduplication
    fingerprint     BLOB    NOT NULL,
    root_id         INTEGER NOT NULL,
    path            TEXT    NOT NULL,
    file_size       INTEGER NOT NULL,
    -- The file's mtime, for detecting changes without reading the whole file (requirements/scanning.md §3.3).
    file_mtime      INTEGER NOT NULL,
    missing_since   INTEGER,
    title           TEXT    NOT NULL,
    sort_key        BLOB    NOT NULL,
    artist_sort_key BLOB    NOT NULL,
    album_sort_key  BLOB    NOT NULL,
    disc_number     INTEGER NOT NULL DEFAULT 0 CHECK (disc_number >= 0),
    track_number    INTEGER,
    track_total     INTEGER,
    disc_total      INTEGER,
    release_date    TEXT    CHECK (release_date GLOB '[0-9][0-9][0-9][0-9]' OR release_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]' OR release_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    explicit        INTEGER NOT NULL DEFAULT 0 CHECK (explicit IN (0, 1)),
    isrc            TEXT,
    -- Other identifier tags, passed through by lowercased tag name for plugins.
    identifiers     TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(identifiers) AND json_type(identifiers) = 'object'),
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
    analyzed_at     INTEGER,
    added_at        INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    UNIQUE (library_id, id),
    FOREIGN KEY (library_id, album_id) REFERENCES albums (library_id, id),
    FOREIGN KEY (library_id, root_id) REFERENCES library_roots (library_id, id)
) STRICT;

CREATE UNIQUE INDEX tracks_file ON tracks (root_id, path);
CREATE INDEX tracks_by_name ON tracks (library_id, sort_key, id);
CREATE INDEX tracks_by_artist ON tracks (library_id, artist_sort_key, sort_key, id);
CREATE INDEX tracks_by_album_order ON tracks (album_id, disc_number, track_number, id);
CREATE INDEX tracks_by_album ON tracks (library_id, album_sort_key, album_id, disc_number, track_number, id);
CREATE INDEX tracks_by_added ON tracks (library_id, added_at, id);
CREATE INDEX tracks_by_release ON tracks (library_id, release_date, id);
CREATE INDEX tracks_by_duration ON tracks (library_id, duration_us, id);
CREATE INDEX tracks_by_fingerprint ON tracks (library_id, fingerprint);
CREATE INDEX tracks_missing ON tracks (library_id, missing_since) WHERE missing_since IS NOT NULL;
CREATE INDEX tracks_unanalyzed ON tracks (library_id, id) WHERE analyzed_at IS NULL;

CREATE TABLE track_artists (
    library_id INTEGER NOT NULL,
    track_id   INTEGER NOT NULL,
    position   INTEGER NOT NULL,
    artist_id  INTEGER NOT NULL,
    -- The artist's name as written in this file
    artist_name TEXT,
    PRIMARY KEY (track_id, position),
    UNIQUE (track_id, artist_id),
    FOREIGN KEY (library_id, track_id) REFERENCES tracks (library_id, id) ON DELETE CASCADE,
    FOREIGN KEY (library_id, artist_id) REFERENCES artists (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX track_artists_by_artist ON track_artists (artist_id, track_id);

-- Album-artist credits as written in each file, resolved into album_artists
CREATE TABLE track_album_artists (
    library_id  INTEGER NOT NULL,
    track_id    INTEGER NOT NULL,
    position    INTEGER NOT NULL,
    artist_id   INTEGER NOT NULL,
    -- The artist's name as written in this file; NULL for the unknown artist
    artist_name TEXT,
    PRIMARY KEY (track_id, position),
    UNIQUE (track_id, artist_id),
    FOREIGN KEY (library_id, track_id) REFERENCES tracks (library_id, id) ON DELETE CASCADE,
    FOREIGN KEY (library_id, artist_id) REFERENCES artists (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX track_album_artists_by_artist ON track_album_artists (artist_id, track_id);

CREATE TABLE track_lyrics (
    library_id INTEGER NOT NULL,
    track_id   INTEGER NOT NULL PRIMARY KEY,
    plain      TEXT,
    synced     TEXT CHECK (synced IS NULL OR (json_valid(synced) AND json_type(synced) = 'array')),
    -- Whether the lyrics are embedded in the track file
    embedded   INTEGER NOT NULL CHECK (embedded IN (0, 1)),
    FOREIGN KEY (library_id, track_id) REFERENCES tracks (library_id, id) ON DELETE CASCADE,
    CHECK (plain IS NOT NULL OR synced IS NOT NULL)
) STRICT;

CREATE TABLE track_waveforms (
    library_id INTEGER NOT NULL,
    track_id   INTEGER NOT NULL PRIMARY KEY,
    data       BLOB    NOT NULL,
    FOREIGN KEY (library_id, track_id) REFERENCES tracks (library_id, id) ON DELETE CASCADE
) STRICT;

-- ───────────────────────────── Tags ─────────────────────────────

CREATE TABLE tags (
    id           INTEGER PRIMARY KEY,
    library_id   INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    type         TEXT    NOT NULL CHECK (type IN ('genre')),
    -- The most-used spelling of track_tags.tag_name
    name         TEXT    NOT NULL,
    -- Folded for case and spacing only (requirements/tags.md §4)
    name_key     TEXT    NOT NULL,
    sort_key     BLOB    NOT NULL,
    track_count  INTEGER NOT NULL DEFAULT 0,
    album_count  INTEGER NOT NULL DEFAULT 0,
    artist_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE (library_id, type, name_key),
    UNIQUE (library_id, id)
) STRICT;

CREATE INDEX tags_by_name ON tags (library_id, type, sort_key, id);
CREATE INDEX tags_by_track_count ON tags (library_id, type, track_count, id);

CREATE TABLE track_tags (
    library_id INTEGER NOT NULL,
    track_id   INTEGER NOT NULL,
    tag_id     INTEGER NOT NULL,
    -- The tag as written in this file
    tag_name   TEXT    NOT NULL,
    PRIMARY KEY (track_id, tag_id),
    FOREIGN KEY (library_id, track_id) REFERENCES tracks (library_id, id) ON DELETE CASCADE,
    FOREIGN KEY (library_id, tag_id) REFERENCES tags (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX track_tags_by_tag ON track_tags (tag_id, track_id);

CREATE TABLE album_tags (
    library_id INTEGER NOT NULL,
    album_id   INTEGER NOT NULL,
    tag_id     INTEGER NOT NULL,
    PRIMARY KEY (album_id, tag_id),
    FOREIGN KEY (library_id, album_id) REFERENCES albums (library_id, id) ON DELETE CASCADE,
    FOREIGN KEY (library_id, tag_id) REFERENCES tags (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX album_tags_by_tag ON album_tags (tag_id, album_id);

CREATE TABLE artist_tags (
    library_id INTEGER NOT NULL,
    artist_id  INTEGER NOT NULL,
    tag_id     INTEGER NOT NULL,
    PRIMARY KEY (artist_id, tag_id),
    FOREIGN KEY (library_id, artist_id) REFERENCES artists (library_id, id) ON DELETE CASCADE,
    FOREIGN KEY (library_id, tag_id) REFERENCES tags (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX artist_tags_by_tag ON artist_tags (tag_id, artist_id);
