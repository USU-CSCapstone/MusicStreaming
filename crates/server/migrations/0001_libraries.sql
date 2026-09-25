-- Libraries and their root folders (requirements/libraries.md §1–§3).

CREATE TABLE libraries (
    id                    INTEGER PRIMARY KEY,
    name                  TEXT    NOT NULL,
    -- Patterns to exclude from scanning, relative to the roots
    excludes              TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(excludes) AND json_type(excludes) = 'array'),
    -- Whether to watch the library for changes
    watch                 INTEGER NOT NULL DEFAULT 1 CHECK (watch IN (0, 1)),
    -- The time between scheduled scans, in minutes; NULL disables them
    scan_interval_minutes INTEGER DEFAULT 1440 CHECK (scan_interval_minutes > 0),
    -- Counts and durations maintained by the scanner in the same transaction as the content it changes
    track_count           INTEGER NOT NULL DEFAULT 0,
    album_count           INTEGER NOT NULL DEFAULT 0,
    artist_count          INTEGER NOT NULL DEFAULT 0,
    duration_us           INTEGER NOT NULL DEFAULT 0,
    -- Oldest library-feed position still answerable; older cursors resync (see 0004).
    feed_horizon          INTEGER NOT NULL DEFAULT 0,
    created_at            INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL
) STRICT;

CREATE TABLE library_roots (
    id         INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
    path       TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    removed_at INTEGER,
    -- Target for tracks' and images' foreign keys.
    UNIQUE (library_id, id)
) STRICT;

-- One active root per path, across all libraries.
CREATE UNIQUE INDEX library_roots_path ON library_roots (path) WHERE removed_at IS NULL;
