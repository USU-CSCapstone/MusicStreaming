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
    width       INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    -- A ThumbHash, filled by the placeholder job after a decode; NULL until then, and empty
    -- when the image could not be decoded
    placeholder BLOB,
    root_id     INTEGER NOT NULL,
    path        TEXT    NOT NULL,
    UNIQUE (library_id, hash),
    UNIQUE (library_id, id),
    FOREIGN KEY (library_id, root_id) REFERENCES library_roots (library_id, id)
) STRICT;

-- The placeholder job's queue
CREATE INDEX images_without_placeholder ON images (library_id) WHERE placeholder IS NULL;

-- Artist biographies, one row per distinct text, found per requirements/scanning.md §3.3
CREATE TABLE biographies (
    id         INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    -- SHA-256 of the text
    hash       BLOB    NOT NULL,
    text       TEXT    NOT NULL,
    UNIQUE (library_id, hash),
    UNIQUE (library_id, id)
) STRICT;

-- ───────────────────────────── Artists ─────────────────────────────

CREATE TABLE artists (
    id               INTEGER PRIMARY KEY,
    library_id       INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    -- NULL is the unknown artist. Otherwise the most-used spelling in artist_spellings
    name             TEXT,
    -- Name split from the tag on semicolons, trimmed, lowercased, ς as σ, whitespace as a space, NFC
    name_key         TEXT    NOT NULL,
    sort_key         BLOB    NOT NULL,
    image_id         INTEGER,
    biography        TEXT,
    album_count      INTEGER NOT NULL DEFAULT 0,
    track_count      INTEGER NOT NULL DEFAULT 0,
    available_track_count INTEGER NOT NULL DEFAULT 0,
    added_at         INTEGER NOT NULL,
    UNIQUE (library_id, name_key),
    UNIQUE (library_id, id),
    FOREIGN KEY (library_id, image_id) REFERENCES images (library_id, id)
) STRICT;

CREATE INDEX artists_by_name ON artists (library_id, sort_key, id);
CREATE INDEX artists_by_added ON artists (library_id, added_at, id);
CREATE INDEX artists_by_image ON artists (library_id, image_id);

-- ───────────────────────────── Albums ─────────────────────────────

CREATE TABLE albums (
    id                    INTEGER PRIMARY KEY,
    library_id            INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    -- NULL is the unknown album. Otherwise the most-used spelling of tracks.album_title
    title                 TEXT,
    -- The title trimmed, lowercased, ς as σ, whitespace as a space, NFC
    title_key             TEXT    NOT NULL,
    -- The album artists' name_keys, sorted so order does not matter, joined with ';'
    artists_key           TEXT    NOT NULL,
    sort_key              BLOB    NOT NULL,
    artist_sort_key       BLOB    NOT NULL,
    type                  TEXT    NOT NULL DEFAULT 'album' CHECK (type IN ('album', 'ep', 'single', 'compilation')),
    -- The label set most of the album's tracks carry, as design/database.md §3 resolves it
    labels                TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(labels) AND json_type(labels) = 'array'),
    image_id              INTEGER,
    release_date          TEXT    CHECK (release_date GLOB '[0-9][0-9][0-9][0-9]' OR release_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]' OR release_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    disc_total            INTEGER,
    -- The sum of album_discs.track_total; NULL when no disc has one
    track_total           INTEGER,
    disc_count            INTEGER NOT NULL DEFAULT 0,
    track_count           INTEGER NOT NULL DEFAULT 0,
    available_track_count INTEGER NOT NULL DEFAULT 0,
    duration_us           INTEGER NOT NULL DEFAULT 0,
    loudness_lufs         REAL,
    peak_dbtp             REAL,
    added_at              INTEGER NOT NULL,
    UNIQUE (library_id, title_key, artists_key),
    UNIQUE (library_id, id),
    FOREIGN KEY (library_id, image_id) REFERENCES images (library_id, id)
) STRICT;

CREATE INDEX albums_by_name ON albums (library_id, sort_key, id);
CREATE INDEX albums_by_artist ON albums (library_id, artist_sort_key, sort_key, id);
CREATE INDEX albums_by_added ON albums (library_id, added_at, id);
CREATE INDEX albums_by_release ON albums (library_id, release_date, sort_key, id);
CREATE INDEX albums_by_duration ON albums (library_id, duration_us, id);
CREATE INDEX albums_by_image ON albums (library_id, image_id);

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

-- How many of the album's tracks credit each artist (track_artists), so an artist's albums are
-- read without their tracks. A row goes with the last of them (0007_catalog_counts.sql).
CREATE TABLE album_credits (
    library_id  INTEGER NOT NULL,
    album_id    INTEGER NOT NULL,
    artist_id   INTEGER NOT NULL,
    track_count INTEGER NOT NULL,
    PRIMARY KEY (album_id, artist_id),
    FOREIGN KEY (library_id, album_id) REFERENCES albums (library_id, id) ON DELETE CASCADE,
    FOREIGN KEY (library_id, artist_id) REFERENCES artists (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX album_credits_by_artist ON album_credits (artist_id, album_id);

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
    -- The album artists' ids in the order this file lists them, each once; resolved onto
    -- album_artists
    album_artist_ids TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(album_artist_ids) AND json_type(album_artist_ids) = 'array'),
    -- The album's title as written in this file
    album_title     TEXT    CHECK (album_title <> ''),
    root_id         INTEGER NOT NULL,
    path            TEXT    NOT NULL,
    file_size       INTEGER NOT NULL,
    -- The file's mtime, for detecting changes without opening the file (requirements/scanning.md §5,
    -- design/scanning.md §2).
    file_mtime      INTEGER NOT NULL,
    missing_since   INTEGER,
    title           TEXT    NOT NULL,
    sort_key        BLOB    NOT NULL,
    -- The first artist's and the album's sort keys, copied so tracks sort by them through one
    -- index, and followed as they change (design/database.md §3)
    artist_sort_key BLOB    NOT NULL,
    album_sort_key  BLOB    NOT NULL,
    -- The first artist's and the album's sort keys as this file gives them, from which the
    -- artist's and the album's are resolved
    file_artist_sort_key BLOB NOT NULL,
    file_album_sort_key  BLOB NOT NULL,
    disc_number     INTEGER NOT NULL DEFAULT 0 CHECK (disc_number >= 0),
    track_number    INTEGER,
    track_total     INTEGER CHECK (track_total >= 0),
    disc_total      INTEGER CHECK (disc_total >= 0),
    release_date    TEXT    CHECK (release_date GLOB '[0-9][0-9][0-9][0-9]' OR release_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]' OR release_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    explicit        INTEGER NOT NULL DEFAULT 0 CHECK (explicit IN (0, 1)),
    -- Per-file album-type inputs, resolved onto albums.type (requirements/albums.md §3)
    compilation     INTEGER NOT NULL DEFAULT 0 CHECK (compilation IN (0, 1)),
    release_type    TEXT    CHECK (release_type <> ''),
    -- The file's record labels, as the scanner reads them; resolved onto albums.labels
    labels          TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(labels) AND json_type(labels) = 'array'),
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
    -- Which analyzer produced the results: 0 until one has, and again once a scan finds the
    -- file's size or mtime changed. Tracks below the current version are reprocessed in the
    -- background (design/scanning.md §12)
    analyzer_version INTEGER NOT NULL DEFAULT 0,
    -- The art this file resolved, embedded or a cover.*, and whether it is embedded; resolved
    -- onto albums.image_id (design/database.md §3)
    image_id        INTEGER,
    image_embedded  INTEGER NOT NULL DEFAULT 0 CHECK (image_embedded IN (0, 1)),
    -- The artist.* and artist.txt this file resolved, resolved onto its album artists
    artist_image_id INTEGER,
    biography_id    INTEGER,
    -- A hash of the path, size and mtime of each sidecar the file resolved, its lyrics file
    -- included, which a scan compares to notice a sidecar change (design/scanning.md §2)
    sidecar_stamp   INTEGER NOT NULL DEFAULT 0,
    added_at        INTEGER NOT NULL,
    UNIQUE (library_id, id),
    FOREIGN KEY (library_id, album_id) REFERENCES albums (library_id, id),
    FOREIGN KEY (library_id, root_id) REFERENCES library_roots (library_id, id),
    FOREIGN KEY (library_id, image_id) REFERENCES images (library_id, id),
    FOREIGN KEY (library_id, artist_image_id) REFERENCES images (library_id, id),
    FOREIGN KEY (library_id, biography_id) REFERENCES biographies (library_id, id)
) STRICT;

CREATE UNIQUE INDEX tracks_file ON tracks (root_id, path);
CREATE INDEX tracks_by_name ON tracks (library_id, sort_key, id);
CREATE INDEX tracks_by_artist ON tracks (library_id, artist_sort_key, sort_key, id);
-- A track with no number sorts last on its disc (requirements/conventions.md §4). The ifnull keeps
-- NULL out of browse paging's keyset, where it would match nothing and pages would skip tracks.
CREATE INDEX tracks_by_album_order ON tracks (album_id, disc_number, ifnull(track_number, 9223372036854775807), id);
CREATE INDEX tracks_by_album ON tracks (library_id, album_sort_key, album_id, disc_number, ifnull(track_number, 9223372036854775807), id);
CREATE INDEX tracks_by_added ON tracks (library_id, added_at, id);
CREATE INDEX tracks_by_release ON tracks (library_id, release_date, album_sort_key, album_id, disc_number, ifnull(track_number, 9223372036854775807), id);
-- Browsing tracks by duration (requirements/conventions.md §4), as albums_by_duration does albums
CREATE INDEX tracks_by_duration ON tracks (library_id, duration_us, id);
-- The sweep's purge, root by root, since a root's last whole walk decides what may go
CREATE INDEX tracks_missing ON tracks (root_id, missing_since) WHERE missing_since IS NOT NULL;
-- The analysis queue in the order it is worked: tracks never analyzed before those an older
-- analyzer did, and newest first within each, so a pick reads the head of one range and sorts
-- nothing.
CREATE INDEX tracks_unanalyzed ON tracks (library_id, analyzer_version, added_at DESC, id)
    WHERE missing_since IS NULL;
-- For the foreign keys' checks as an image or biography is deleted
CREATE INDEX tracks_by_image ON tracks (library_id, image_id) WHERE image_id IS NOT NULL;
CREATE INDEX tracks_by_artist_image ON tracks (library_id, artist_image_id)
    WHERE artist_image_id IS NOT NULL;
CREATE INDEX tracks_by_biography ON tracks (library_id, biography_id) WHERE biography_id IS NOT NULL;
-- An album's tracks offering art, in the order design/database.md §3 gives, so an album reads
-- the first from one entry
CREATE INDEX tracks_with_art ON tracks (album_id, missing_since IS NOT NULL, image_embedded DESC,
    disc_number, ifnull(track_number, 9223372036854775807), path, id) WHERE image_id IS NOT NULL;
-- An album's present tracks offering an artist image or a biography, which an artist's query
-- names (INDEXED BY) so it reads one entry per album whatever the planner's statistics
CREATE INDEX tracks_with_artist_image ON tracks (album_id, artist_image_id)
    WHERE artist_image_id IS NOT NULL AND missing_since IS NULL;
CREATE INDEX tracks_with_biography ON tracks (album_id, biography_id)
    WHERE biography_id IS NOT NULL AND missing_since IS NULL;
-- An album's analyzed tracks with the columns its loudness reads, so computing it reads no
-- table rows
CREATE INDEX tracks_analyzed ON tracks (album_id, duration_us, loudness_lufs, peak_dbtp)
    WHERE loudness_lufs IS NOT NULL;

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
    FOREIGN KEY (library_id, track_id) REFERENCES tracks (library_id, id) ON DELETE CASCADE,
    -- Exactly one form: the plain text, or the timed lines alone
    CHECK (plain IS NOT NULL OR synced IS NOT NULL),
    CHECK (synced IS NULL OR plain IS NULL)
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
    -- The most-used spelling in tag_spellings
    name         TEXT    NOT NULL,
    -- The name folded as artists.name_key is, with whitespace runs collapsed (requirements/tags.md §4)
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

-- An album's genres are its tracks' (requirements/tags.md §6), and an artist's are their
-- credited tracks'. Each row counts the tracks that bring the genre, so it goes with the last
-- of them (0007_catalog_counts.sql).
CREATE TABLE album_tags (
    library_id  INTEGER NOT NULL,
    album_id    INTEGER NOT NULL,
    tag_id      INTEGER NOT NULL,
    track_count INTEGER NOT NULL,
    PRIMARY KEY (album_id, tag_id),
    FOREIGN KEY (library_id, album_id) REFERENCES albums (library_id, id) ON DELETE CASCADE,
    FOREIGN KEY (library_id, tag_id) REFERENCES tags (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX album_tags_by_tag ON album_tags (tag_id, album_id);

CREATE TABLE artist_tags (
    library_id  INTEGER NOT NULL,
    artist_id   INTEGER NOT NULL,
    tag_id      INTEGER NOT NULL,
    track_count INTEGER NOT NULL,
    PRIMARY KEY (artist_id, tag_id),
    FOREIGN KEY (library_id, artist_id) REFERENCES artists (library_id, id) ON DELETE CASCADE,
    FOREIGN KEY (library_id, tag_id) REFERENCES tags (library_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX artist_tags_by_tag ON artist_tags (tag_id, artist_id);
